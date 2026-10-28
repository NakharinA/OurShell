//! End-to-end test for Part B (team vaults) and Part A (password change /
//! recovery while signed in), against a real server (see `sync_support`).
//!
//! Skips (prints why, passes) if no test Postgres is reachable.

mod sync_support;

use ourshell_core::sync;
use ourshell_core::vault::{Host, Vault};

#[tokio::test(flavor = "multi_thread")]
async fn team_vault_share_edit_and_remove_member() {
    let Some(server) = sync_support::start().await else {
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    let path_a = dir.path().join("a.db");
    let path_b = dir.path().join("b.db");
    let password = "correct horse battery staple";

    // --- A and B each sign up (open signups, no invite needed here).
    let created_a = Vault::create(&path_a, password, true).unwrap();
    let mut vault_a = created_a.vault;
    let mut session_a = sync::signup(
        &server.base_url,
        "a@example.com",
        password,
        &mut vault_a,
        "device-a",
        None,
    )
    .await
    .expect("a signup");

    let created_b = Vault::create(&path_b, password, true).unwrap();
    let mut vault_b = created_b.vault;
    let mut session_b = sync::signup(
        &server.base_url,
        "b@example.com",
        password,
        &mut vault_b,
        "device-b",
        None,
    )
    .await
    .expect("b signup");

    // --- A creates a team vault with B as a member.
    let team = sync::create_team(
        &mut vault_a,
        &session_a,
        "Ops",
        &["b@example.com".to_string()],
    )
    .await
    .expect("create team");
    assert_eq!(team.kind, "team");

    // --- A puts a host directly into the team vault and syncs.
    let host_id = vault_a
        .put_in(
            &team.vault_id,
            None,
            &Host {
                name: "shared-web".into(),
                hostname: "198.51.100.9".into(),
                port: 22,
                ..Default::default()
            },
        )
        .unwrap();
    let report = sync::sync(&mut vault_a, &mut session_a)
        .await
        .expect("a sync");
    assert_eq!(report.pushed, 1);

    // --- B syncs: picks up the new team vault (via account.vaults) and
    // pulls + decrypts the shared host.
    let report = sync::sync(&mut vault_b, &mut session_b)
        .await
        .expect("b sync (1)");
    assert_eq!(report.pulled, 1);
    assert_eq!(vault_b.get::<Host>(&host_id).unwrap().name, "shared-web");
    assert!(
        vault_b.list::<Host>().unwrap().is_empty(),
        "the shared host must not show up in B's *personal* vault listing"
    );
    assert_eq!(vault_b.list_all::<Host>().unwrap().len(), 1);

    // --- B edits the host and syncs; A must accept B's signature.
    let mut edited = vault_b.get::<Host>(&host_id).unwrap();
    edited.port = 2222;
    vault_b
        .put_in(&team.vault_id, Some(&host_id), &edited)
        .unwrap();
    let report = sync::sync(&mut vault_b, &mut session_b)
        .await
        .expect("b sync (2)");
    assert_eq!(report.pushed, 1);

    let report = sync::sync(&mut vault_a, &mut session_a)
        .await
        .expect("a sync (2)");
    assert_eq!(report.pulled, 1);
    assert_eq!(
        vault_a.get::<Host>(&host_id).unwrap().port,
        2222,
        "A must accept B's signed edit"
    );

    // --- A removes B from the team (rotates the key) and pushes the
    // re-sealed items right away.
    sync::team_remove_member(
        &mut vault_a,
        &mut session_a,
        &team.vault_id,
        "b@example.com",
    )
    .await
    .expect("remove member");

    // --- B syncs again: loses the vault (no longer a member) and cannot
    // read anything from it any more.
    let report = sync::sync(&mut vault_b, &mut session_b)
        .await
        .expect("b sync (3)");
    let _ = report;
    assert!(
        vault_b.list_all::<Host>().unwrap().is_empty(),
        "B must lose the team vault entirely once removed"
    );

    // --- A can still read everything (rotation didn't lock A out).
    assert_eq!(vault_a.get::<Host>(&host_id).unwrap().port, 2222);
}

#[tokio::test(flavor = "multi_thread")]
async fn share_personal_item_into_team() {
    let Some(server) = sync_support::start().await else {
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    let path_a = dir.path().join("a.db");
    let path_b = dir.path().join("b.db");
    let password = "correct horse battery staple";

    let created_a = Vault::create(&path_a, password, true).unwrap();
    let mut vault_a = created_a.vault;
    let host_id = vault_a
        .put(
            None,
            &Host {
                name: "personal-db".into(),
                hostname: "198.51.100.20".into(),
                port: 5432,
                ..Default::default()
            },
        )
        .unwrap();
    let mut session_a = sync::signup(
        &server.base_url,
        "owner@example.com",
        password,
        &mut vault_a,
        "device-a",
        None,
    )
    .await
    .expect("signup");
    sync::sync(&mut vault_a, &mut session_a)
        .await
        .expect("initial sync");

    let created_b = Vault::create(&path_b, password, true).unwrap();
    let mut vault_b = created_b.vault;
    let mut session_b = sync::signup(
        &server.base_url,
        "mate@example.com",
        password,
        &mut vault_b,
        "device-b",
        None,
    )
    .await
    .expect("b signup");

    let team = sync::create_team(
        &mut vault_a,
        &session_a,
        "Infra",
        &["mate@example.com".to_string()],
    )
    .await
    .expect("create team");

    let (new_id, _report) =
        sync::share_item(&mut vault_a, &mut session_a, &host_id, &team.vault_id)
            .await
            .expect("share item");
    assert_ne!(
        new_id, host_id,
        "a moved item gets a fresh id (its AAD binds the old one)"
    );

    // The old id is gone from A's personal vault...
    assert!(vault_a.get::<Host>(&host_id).is_err());
    assert!(vault_a.list::<Host>().unwrap().is_empty());
    // ...and the new id lives in the team vault instead.
    assert_eq!(vault_a.get::<Host>(&new_id).unwrap().name, "personal-db");

    let report = sync::sync(&mut vault_b, &mut session_b)
        .await
        .expect("b sync");
    assert!(report.pulled >= 1);
    assert_eq!(vault_b.get::<Host>(&new_id).unwrap().name, "personal-db");
}

#[tokio::test(flavor = "multi_thread")]
async fn password_change_then_login_elsewhere() {
    let Some(server) = sync_support::start().await else {
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    let path_a = dir.path().join("a.db");
    let old_password = "correct horse battery staple";
    let new_password = "donkey mountain ladder zephyr";

    let created = Vault::create(&path_a, old_password, true).unwrap();
    let mut vault = created.vault;
    let id = vault
        .put(
            None,
            &Host {
                name: "x".into(),
                hostname: "10.0.0.1".into(),
                port: 22,
                ..Default::default()
            },
        )
        .unwrap();
    let mut session = sync::signup(
        &server.base_url,
        "pw-change@example.com",
        old_password,
        &mut vault,
        "device-a",
        None,
    )
    .await
    .expect("signup");
    sync::sync(&mut vault, &mut session)
        .await
        .expect("initial sync");

    sync::change_password(&mut vault, Some(&session), new_password)
        .await
        .expect("change password");

    // Old password no longer unlocks this vault.
    vault.lock();
    assert!(vault.unlock(old_password).is_err());
    vault.unlock(new_password).unwrap();
    assert_eq!(vault.get::<Host>(&id).unwrap().name, "x");

    // A brand-new device can log in with the NEW password...
    let path_c = dir.path().join("c.db");
    let (mut vault_c, mut session_c) = sync::login(
        &server.base_url,
        "pw-change@example.com",
        new_password,
        &path_c,
        "device-c",
        None,
    )
    .await
    .expect("login with new password");
    sync::sync(&mut vault_c, &mut session_c)
        .await
        .expect("c sync");
    assert_eq!(vault_c.get::<Host>(&id).unwrap().name, "x");

    // ...but NOT with the old one.
    let path_d = dir.path().join("d.db");
    let err = sync::login(
        &server.base_url,
        "pw-change@example.com",
        old_password,
        &path_d,
        "device-d",
        None,
    )
    .await;
    assert!(err.is_err(), "the old password must be rejected");

    // The original device's sync still works after a password change
    // (its own session wasn't revoked).
    let report = sync::sync(&mut vault, &mut session)
        .await
        .expect("a still syncs");
    let _ = report;
}

#[tokio::test(flavor = "multi_thread")]
async fn recovery_while_signed_in_keeps_syncing() {
    let Some(server) = sync_support::start().await else {
        return;
    };

    let dir = tempfile::tempdir().unwrap();
    let path_a = dir.path().join("a.db");
    let path_b = dir.path().join("b.db");
    let password = "correct horse battery staple";

    let created = Vault::create(&path_a, password, true).unwrap();
    let mut vault_a = created.vault;
    let recovery_phrase = created.recovery_phrase.to_string();
    let host_id = vault_a
        .put(
            None,
            &Host {
                name: "recoverable".into(),
                hostname: "10.0.0.9".into(),
                port: 22,
                ..Default::default()
            },
        )
        .unwrap();
    let mut session_a = sync::signup(
        &server.base_url,
        "recover@example.com",
        password,
        &mut vault_a,
        "device-a",
        None,
    )
    .await
    .expect("signup");
    sync::sync(&mut vault_a, &mut session_a)
        .await
        .expect("initial sync");

    // A second device, logged in before the recovery, will need to keep
    // verifying A's items afterward.
    let (mut vault_b, mut session_b) = sync::login(
        &server.base_url,
        "recover@example.com",
        password,
        &path_b,
        "device-b",
        None,
    )
    .await
    .expect("b login");
    sync::sync(&mut vault_b, &mut session_b)
        .await
        .expect("b sync (1)");
    assert_eq!(vault_b.get::<Host>(&host_id).unwrap().name, "recoverable");

    // A recovers (forgot the password) while still signed in, and pushes
    // the account key material to the server.
    let new_password = "zebra violin orchard tunnel";
    vault_a
        .recover(&recovery_phrase, new_password)
        .expect("local recovery");
    let material = sync::AccountKeyMaterial::from(&vault_a);
    sync::push_account_keys(&material, &session_a, new_password)
        .await
        .expect("push recovered account keys");

    // A's own session keeps working (same account keys -- no re-signing
    // happened, since this vault has `recovery_account_wrap`).
    let report = sync::sync(&mut vault_a, &mut session_a)
        .await
        .expect("a syncs after recovery");
    let _ = report;

    // B's session was revoked by the password/recovery push (every OTHER
    // session for the account is revoked -- the normal "signed out
    // everywhere" security behaviour of a password reset).
    let err = sync::sync(&mut vault_b, &mut session_b).await;
    assert!(
        matches!(err, Err(sync::SyncError::Api { status: 401, .. })),
        "B's old session must be revoked, got {err:?}"
    );

    // A edits the item post-recovery and pushes.
    let mut edited = vault_a.get::<Host>(&host_id).unwrap();
    edited.port = 2022;
    vault_a.put(Some(&host_id), &edited).unwrap();
    sync::sync(&mut vault_a, &mut session_a)
        .await
        .expect("a push after recovery edit");

    // B logs in again with the NEW password on a fresh device, and
    // immediately sees and verifies A's edit -- with the same account
    // signing key as before the recovery, no special handling needed
    // (this is the gap this feature closes: see docs/self-hosting.md).
    let path_b2 = dir.path().join("b2.db");
    let (mut vault_b2, mut session_b2) = sync::login(
        &server.base_url,
        "recover@example.com",
        new_password,
        &path_b2,
        "device-b2",
        None,
    )
    .await
    .expect("b logs in again with the new password");
    sync::sync(&mut vault_b2, &mut session_b2)
        .await
        .expect("b2 sync");
    assert_eq!(vault_b2.get::<Host>(&host_id).unwrap().port, 2022);
}
