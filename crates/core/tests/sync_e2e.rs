//! End-to-end sync test: boots a real server (see `sync_support`) against
//! the test Postgres and drives two independent local vaults ("device A"
//! and "device B") through signup, a new-device login, a push/pull
//! conflict, and a tombstone propagating -- exactly the flow described in
//! the project plan's "Adding a new device" section.
//!
//! Skips (prints why, passes) if no test Postgres is reachable.

mod sync_support;

use ourshell_core::sync;
use ourshell_core::vault::{Host, Vault};
use sqlx_core::query_as::query_as;

#[tokio::test(flavor = "multi_thread")]
async fn full_sync_flow_two_devices_conflict_and_tombstone() {
    let Some(server) = sync_support::start().await else {
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    let path_a = dir.path().join("a.db");
    let path_b = dir.path().join("b.db");
    let password = "correct horse battery staple";

    // --- Device A: create a local (not-yet-synced) vault with two hosts.
    let created = Vault::create(&path_a, password, true).unwrap();
    let mut vault_a = created.vault;
    let host1_id = vault_a
        .put(
            None,
            &Host {
                name: "prod-web".into(),
                hostname: "198.51.100.42".into(),
                port: 22,
                notes: Some("super-secret-marker-prod".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let host2_id = vault_a
        .put(
            None,
            &Host {
                name: "staging-db".into(),
                hostname: "198.51.100.77".into(),
                port: 5432,
                ..Default::default()
            },
        )
        .unwrap();

    // --- Signs up: uploads the vault's existing key material and the two hosts.
    let mut session_a = sync::signup(
        &server.base_url,
        "alice@example.com",
        password,
        &mut vault_a,
        "device-a",
        None,
    )
    .await
    .expect("signup");
    assert!(
        !vault_a.header().local_only,
        "signup marks the vault as syncing"
    );

    let report = sync::sync(&mut vault_a, &mut session_a)
        .await
        .expect("sync a (1)");
    assert_eq!(report.pushed, 2);
    assert_eq!(report.conflicts, 0);

    // --- Device B: brand-new machine, no local vault yet. Logs in and
    // pulls both hosts down, decrypted.
    let (mut vault_b, mut session_b) = sync::login(
        &server.base_url,
        "alice@example.com",
        password,
        &path_b,
        "device-b",
        None,
    )
    .await
    .expect("login on a new device");
    let report = sync::sync(&mut vault_b, &mut session_b)
        .await
        .expect("sync b (1)");
    assert_eq!(report.pulled, 2);
    assert_eq!(report.conflicts, 0);

    assert_eq!(vault_b.list::<Host>().unwrap().len(), 2);
    let web_on_b = vault_b.get::<Host>(&host1_id).unwrap();
    assert_eq!(web_on_b.hostname, "198.51.100.42");
    assert_eq!(web_on_b.notes.as_deref(), Some("super-secret-marker-prod"));

    // --- B edits host1 and syncs up first.
    let mut web_on_b = vault_b.get::<Host>(&host1_id).unwrap();
    web_on_b.port = 2222;
    vault_b.put(Some(&host1_id), &web_on_b).unwrap();
    let report = sync::sync(&mut vault_b, &mut session_b)
        .await
        .expect("sync b (2)");
    assert_eq!(report.pushed, 1);
    assert_eq!(report.conflicts, 0);

    // --- A, offline since its first sync, edits the SAME host1 and then syncs.
    let mut web_on_a = vault_a.get::<Host>(&host1_id).unwrap();
    web_on_a.notes = Some("edited-offline-on-a".into());
    vault_a.put(Some(&host1_id), &web_on_a).unwrap();
    let report = sync::sync(&mut vault_a, &mut session_a)
        .await
        .expect("sync a (2)");
    assert_eq!(
        report.conflicts, 1,
        "A's offline edit should conflict with B's already-pushed edit"
    );

    // The server's copy (B's edit) wins for host1's id...
    let web_after = vault_a.get::<Host>(&host1_id).unwrap();
    assert_eq!(web_after.port, 2222, "B's port change won the conflict");
    assert_eq!(
        web_after.notes.as_deref(),
        Some("super-secret-marker-prod"),
        "B never touched notes, so A's conflicting edit to it must not have won"
    );
    // ...but A's edit is not lost: it exists as a renamed conflict copy.
    let hosts_on_a = vault_a.list::<Host>().unwrap();
    let copy = hosts_on_a
        .iter()
        .find(|(_, h)| h.name == "prod-web (conflict copy)")
        .expect("a's conflicting edit should survive as a renamed conflict copy");
    assert_eq!(copy.1.notes.as_deref(), Some("edited-offline-on-a"));

    // --- A deletes host2; the tombstone propagates to B.
    vault_a.delete(&host2_id).unwrap();
    let report = sync::sync(&mut vault_a, &mut session_a)
        .await
        .expect("sync a (3)");
    assert_eq!(report.pushed, 1);

    let report = sync::sync(&mut vault_b, &mut session_b)
        .await
        .expect("sync b (3)");
    assert!(
        report.pulled >= 1,
        "the tombstone and the conflict copy should pull down"
    );
    assert!(
        vault_b.get::<Host>(&host2_id).is_err(),
        "the deleted host must be gone on B too"
    );
    assert!(vault_b
        .metas()
        .unwrap()
        .iter()
        .any(|m| m.id == host2_id && m.deleted));
    assert!(
        vault_b
            .list::<Host>()
            .unwrap()
            .iter()
            .any(|(_, h)| h.name == "prod-web (conflict copy)"),
        "B should also pull the conflict copy A created"
    );

    // --- Nothing readable ever touches the database: query `items` directly.
    let vault_id = uuid::Uuid::parse_str(&vault_a.header().org_id).unwrap();
    let rows: Vec<(Vec<u8>,)> =
        query_as::<_, (Vec<u8>,)>("SELECT envelope FROM items WHERE vault_id = $1")
            .bind(vault_id)
            .fetch_all(&server.pool)
            .await
            .expect("query items table");
    assert!(!rows.is_empty());
    for (raw,) in rows {
        let text = String::from_utf8_lossy(&raw);
        assert!(
            !text.contains("198.51.100"),
            "hostname leaked into the DB column"
        );
        assert!(
            !text.contains("super-secret-marker-prod"),
            "notes leaked into the DB column"
        );
        assert!(
            !text.contains("edited-offline-on-a"),
            "conflicting edit leaked into the DB column"
        );
        assert!(
            !text.contains("prod-web"),
            "host name leaked into the DB column"
        );
    }
}
