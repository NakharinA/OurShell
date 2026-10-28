//! CSV importer modeled on Termius's host export/import template.

use std::collections::HashMap;

use super::{ImportOptions, ImportReport, Result};
use crate::vault::{Group, Host, Identity, SshKey, Vault};

/// One row parsed from a Termius-style host CSV.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedCsvHost {
    /// Nested group path, e.g. `["Work", "Prod"]` from a `Work/Prod` cell.
    pub groups: Vec<String>,
    pub label: String,
    pub tags: Vec<String>,
    pub hostname: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    /// Inline private key text, if the row carried one.
    pub key_text: Option<String>,
}

fn find_col(headers: &[String], candidates: &[&str]) -> Option<usize> {
    headers
        .iter()
        .position(|h| candidates.iter().any(|c| h.eq_ignore_ascii_case(c)))
}

/// Parses a Termius-style CSV export. The header row's column names are
/// matched case-insensitively against the usual Termius template names;
/// unrecognized columns are ignored. Only rows whose `Protocol` column (when
/// present) is `ssh` are imported.
pub fn parse_termius_csv(data: &str) -> Result<Vec<ParsedCsvHost>> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(data.as_bytes());
    let headers: Vec<String> = reader.headers()?.iter().map(str::to_string).collect();

    let idx_groups = find_col(&headers, &["groups", "group"]);
    let idx_label = find_col(&headers, &["label", "name"]);
    let idx_tags = find_col(&headers, &["tags"]);
    let idx_hostname = find_col(&headers, &["hostname/ip", "hostname", "address"]);
    let idx_protocol = find_col(&headers, &["protocol"]);
    let idx_port = find_col(&headers, &["port"]);
    let idx_username = find_col(&headers, &["username"]);
    let idx_password = find_col(&headers, &["password"]);
    let idx_key = find_col(&headers, &["ssh_key", "key"]);

    let mut out = Vec::new();
    for record in reader.records() {
        let record = record?;
        let get = |idx: Option<usize>| -> Option<&str> {
            idx.and_then(|i| record.get(i))
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };

        if let Some(proto) = get(idx_protocol) {
            if !proto.eq_ignore_ascii_case("ssh") {
                continue;
            }
        }
        let Some(label) = get(idx_label) else {
            continue;
        };
        let Some(hostname) = get(idx_hostname) else {
            continue;
        };

        let groups = get(idx_groups)
            .map(|g| {
                g.split('/')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let tags = get(idx_tags)
            .map(|t| {
                t.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let port = get(idx_port).and_then(|p| p.parse().ok()).unwrap_or(22);

        out.push(ParsedCsvHost {
            groups,
            label: label.to_string(),
            tags,
            hostname: hostname.to_string(),
            port,
            username: get(idx_username).map(str::to_string),
            password: get(idx_password).map(str::to_string),
            key_text: get(idx_key).map(str::to_string),
        });
    }
    Ok(out)
}

/// A plan to import a set of parsed CSV rows. Building the plan does no
/// I/O; [`apply`] does the actual vault writes.
#[derive(Debug, Clone, Default)]
pub struct ImportPlan {
    pub hosts: Vec<ParsedCsvHost>,
}

pub fn plan_termius_csv_import(parsed: Vec<ParsedCsvHost>) -> ImportPlan {
    ImportPlan { hosts: parsed }
}

/// Imports a plan into `vault`. Creates `Group`s for nested group paths
/// (deduped against existing groups and across rows sharing a prefix),
/// `Host`s, and `Identity`s for any username/password/inline key. Hosts
/// whose name already exists are skipped (and reported).
pub fn apply(
    vault: &mut Vault,
    plan: &ImportPlan,
    _options: &ImportOptions,
) -> Result<ImportReport> {
    let mut report = ImportReport::default();
    let mut group_cache: HashMap<Vec<String>, String> = HashMap::new();

    for row in &plan.hosts {
        if vault.find::<Host>(&row.label).is_ok() {
            report.hosts_skipped.push(row.label.clone());
            continue;
        }

        let group_id = if row.groups.is_empty() {
            None
        } else {
            Some(group_chain_id(
                vault,
                &row.groups,
                &mut group_cache,
                &mut report,
            )?)
        };

        let identity_id =
            if row.username.is_some() || row.password.is_some() || row.key_text.is_some() {
                let key_id = match &row.key_text {
                    Some(key_text) => match crate::keygen::import(key_text, None) {
                        Ok(imported) => {
                            let item = imported.to_vault_item(&format!("{}-key", row.label), None);
                            let id = vault.put(None, &item)?;
                            report.keys_added += 1;
                            Some(id)
                        }
                        Err(crate::keygen::KeygenError::PassphraseRequired) => {
                            report.warnings.push(format!(
                                "{}: inline key is encrypted; no passphrase supplied, stored as-is",
                                row.label
                            ));
                            let item = SshKey {
                                name: format!("{}-key", row.label),
                                private_key: key_text.clone(),
                                public_key: crate::keygen::peek_public_key(key_text)
                                    .unwrap_or_default(),
                                passphrase: None,
                            };
                            let id = vault.put(None, &item)?;
                            report.keys_added += 1;
                            Some(id)
                        }
                        Err(e) => {
                            report
                                .warnings
                                .push(format!("{}: could not import inline key: {}", row.label, e));
                            None
                        }
                    },
                    None => None,
                };
                let identity = Identity {
                    name: format!("{}-identity", row.label),
                    username: row.username.clone(),
                    password: row.password.clone(),
                    key_id,
                };
                let id = vault.put(None, &identity)?;
                report.identities_added += 1;
                Some(id)
            } else {
                None
            };

        let host = Host {
            name: row.label.clone(),
            hostname: row.hostname.clone(),
            port: row.port,
            username: row.username.clone(),
            identity_id,
            group_id,
            tags: row.tags.clone(),
            ..Default::default()
        };
        vault.put(None, &host)?;
        report.hosts_added += 1;
    }

    Ok(report)
}

/// Finds or creates the `Group` chain for a nested path like
/// `["Work", "Prod"]`, returning the id of the leaf (`Prod`).
fn group_chain_id(
    vault: &mut Vault,
    path: &[String],
    cache: &mut HashMap<Vec<String>, String>,
    report: &mut ImportReport,
) -> Result<String> {
    if let Some(id) = cache.get(path) {
        return Ok(id.clone());
    }
    let parent_id = if path.len() > 1 {
        Some(group_chain_id(
            vault,
            &path[..path.len() - 1],
            cache,
            report,
        )?)
    } else {
        None
    };
    let name = path.last().expect("non-empty path").clone();
    let existing = vault
        .list::<Group>()?
        .into_iter()
        .find(|(_, g)| g.name.eq_ignore_ascii_case(&name) && g.parent_id == parent_id);
    let id = if let Some((id, _)) = existing {
        id
    } else {
        let id = vault.put(None, &Group { name, parent_id })?;
        report.groups_added += 1;
        id
    };
    cache.insert(path.to_vec(), id.clone());
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::kdf::KdfParams;
    use crate::vault::Created;
    use std::path::Path;

    fn new_vault(dir: &Path) -> Created {
        Vault::create_with_kdf(
            &dir.join("v.db"),
            "pw",
            true,
            KdfParams::insecure_for_tests(),
        )
        .unwrap()
    }

    const CSV: &str = "Groups,Label,Tags,Hostname/IP,Protocol,Port,Username,Password,SSH_KEY\n\
Work/Prod,web1,prod;web,10.0.0.1,ssh,22,root,s3cret,\n\
Work/Prod,web2,prod,10.0.0.2,ssh,2222,root,,\n\
Work/Staging,db1,db,10.0.0.3,ssh,,admin,,\n\
Personal,laptop,,192.168.1.1,rdp,3389,me,,\n";

    #[test]
    fn parses_groups_tags_and_protocol_filter() {
        let rows = parse_termius_csv(CSV).unwrap();
        // The `rdp` row must be filtered out.
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].groups, vec!["Work", "Prod"]);
        assert_eq!(rows[0].label, "web1");
        assert_eq!(rows[0].tags, vec!["prod;web"]);
        assert_eq!(rows[0].hostname, "10.0.0.1");
        assert_eq!(rows[0].port, 22);
        assert_eq!(rows[0].username, Some("root".into()));
        assert_eq!(rows[0].password, Some("s3cret".into()));
        assert_eq!(rows[2].port, 22, "missing port falls back to 22");
    }

    #[test]
    fn apply_creates_nested_groups_hosts_and_identities() {
        let rows = parse_termius_csv(CSV).unwrap();
        let plan = plan_termius_csv_import(rows);

        let dir = tempfile::tempdir().unwrap();
        let Created { mut vault, .. } = new_vault(dir.path());
        let report = apply(&mut vault, &plan, &ImportOptions::default()).unwrap();

        assert_eq!(report.hosts_added, 3);
        assert_eq!(report.identities_added, 3);
        // Work, Prod, Staging: 3 distinct groups, Prod shared by web1/web2.
        assert_eq!(report.groups_added, 3);
        assert_eq!(vault.list::<Group>().unwrap().len(), 3);

        let (_, web1) = vault.find::<Host>("web1").unwrap();
        let (_, web2) = vault.find::<Host>("web2").unwrap();
        assert_eq!(web1.group_id, web2.group_id, "same nested group is reused");

        let groups = vault.list::<Group>().unwrap();
        let (work_id, _) = groups.iter().find(|(_, g)| g.name == "Work").unwrap();
        let (_, prod_group) = groups.iter().find(|(_, g)| g.name == "Prod").unwrap();
        assert_eq!(prod_group.parent_id, Some(work_id.clone()));
    }

    #[test]
    fn apply_imports_inline_key() {
        let generated =
            crate::keygen::generate(crate::keygen::KeyAlgorithm::Ed25519, "t", None).unwrap();
        // The inline key is multi-line; the `csv` crate handles a quoted
        // field containing literal newlines natively.
        let csv_quoted = format!(
            "Label,Hostname,Protocol,Username,Key\nweb1,10.0.0.1,ssh,root,\"{}\"\n",
            generated.private_key_openssh.as_str()
        );

        let rows = parse_termius_csv(&csv_quoted).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].key_text.is_some());

        let plan = plan_termius_csv_import(rows);
        let dir = tempfile::tempdir().unwrap();
        let Created { mut vault, .. } = new_vault(dir.path());
        let report = apply(&mut vault, &plan, &ImportOptions::default()).unwrap();

        assert_eq!(report.keys_added, 1);
        let (_, key) = &vault.list::<SshKey>().unwrap()[0];
        assert_eq!(key.public_key, generated.public_key_openssh);
    }

    #[test]
    fn apply_skips_existing_host() {
        let dir = tempfile::tempdir().unwrap();
        let Created { mut vault, .. } = new_vault(dir.path());
        vault
            .put(
                None,
                &Host {
                    name: "web1".into(),
                    hostname: "already-here".into(),
                    ..Default::default()
                },
            )
            .unwrap();

        let rows = parse_termius_csv(CSV).unwrap();
        let plan = plan_termius_csv_import(rows);
        let report = apply(&mut vault, &plan, &ImportOptions::default()).unwrap();

        assert_eq!(report.hosts_skipped, vec!["web1".to_string()]);
        assert_eq!(report.hosts_added, 2);
    }
}
