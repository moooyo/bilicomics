use super::*;
use crate::source::AccountIdentity;

fn offline_options(data_dir: &Path) -> AppOptions {
    AppOptions {
        data_dir: data_dir.to_owned(),
        profile: DeviceProfile::Paperwhite5,
        demo: false,
        offline: true,
        initial_screen: String::new(),
        landscape: false,
    }
}

fn synthetic_session(account_id: &str, credential: &str) -> Result<AuthSession> {
    Ok(AuthSession::from_cookie_text(&format!(
        "SESSDATA={credential}; bili_jct=synthetic-csrf; DedeUserID={account_id}"
    ))?
    .with_identity(
        AccountIdentity {
            id: account_id.into(),
            name: "Synthetic persisted account".into(),
            avatar_url: String::new(),
        },
        1_800_000_000,
    )?)
}

#[test]
fn prepared_production_session_updates_and_logout_survive_restart() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let session_file = SessionFile::new(&directory.path().join("account"));
    session_file.save(&synthetic_session("42", "synthetic-initial-session")?)?;

    let prepared = App::prepare(offline_options(directory.path()))?;
    let mut updated = prepared.client.session().context("Load the saved synthetic session")?;
    updated.apply_set_cookie(
        &[
            (
                "set-cookie".into(),
                "SESSDATA=synthetic-updated-session; Domain=.bilibili.com; Path=/".into(),
            ),
            (
                "set-cookie".into(),
                "XSRF-TOKEN=synthetic%2Bxsrf; Domain=manga.bilibili.com; Path=/".into(),
            ),
        ],
        "manga.bilibili.com",
        1_800_000_001,
    )?;
    prepared.client.replace_session(Some(updated.clone()))?;
    assert_eq!(session_file.load()?, Some(updated.clone()));
    drop(prepared);

    let restarted = App::prepare(offline_options(directory.path()))?;
    assert_eq!(restarted.client.session(), Some(updated));
    restarted.client.replace_session(None)?;
    assert!(session_file.load()?.is_none());
    drop(restarted);

    let signed_out = App::prepare(offline_options(directory.path()))?;
    assert!(signed_out.client.session().is_none());
    Ok(())
}

#[test]
fn an_isolated_import_client_cannot_replace_or_clear_the_production_session() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let session_file = SessionFile::new(&directory.path().join("account"));
    let production_session = synthetic_session("42", "synthetic-production-session")?;
    session_file.save(&production_session)?;
    let prepared = App::prepare(offline_options(directory.path()))?;

    let mut imported = synthetic_session("43", "synthetic-import-session")?;
    let temporary = BiliClient::new(Some(imported.clone()), directory.path())?;
    imported.apply_set_cookie(
        &[(
            "set-cookie".into(),
            "SESSDATA=synthetic-validated-import; Domain=.bilibili.com; Path=/".into(),
        )],
        "api.bilibili.com",
        1_800_000_001,
    )?;
    temporary.replace_session(Some(imported))?;
    assert_eq!(session_file.load()?, Some(production_session.clone()));

    temporary.replace_session(None)?;
    assert_eq!(session_file.load()?, Some(production_session.clone()));
    assert_eq!(prepared.client.session(), Some(production_session));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn rejected_logout_preserves_the_live_session_and_restart_rejects_unsafe_storage() -> Result<()> {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir()?;
    let file = SessionFile::new(&directory.path().join("account"));
    let old = synthetic_session("11", "test-before-switch")?;
    let next = synthetic_session("43", "test-after-switch")?;
    file.save(&old)?;
    let prepared = App::prepare(offline_options(directory.path()))?;
    assert!(prepared.client.session() == Some(old));
    let mut app = super::super::test_support::app(directory.path(), DeviceProfile::Paperwhite5,
        prepared.client, tokio::runtime::Handle::current())?;
    app.options.offline = true;
    app.online = false;
    app.switch_session(next.clone())?;
    assert_eq!(app.account, "bili_43");
    let restart = || App::prepare(offline_options(directory.path()));
    assert!(restart()?.client.session() == Some(next.clone()));

    let private = file.path().parent().unwrap().to_owned();
    let retained = private.with_file_name("retained-private");
    std::fs::rename(&private, &retained)?;
    symlink(&retained, &private)?;
    let epoch = app.epoch;
    assert!(app.logout().is_err());
    assert_eq!(app.epoch, epoch);
    assert!(app.client.session() == Some(next.clone()));
    assert!(restart().is_err());
    std::fs::remove_file(&private)?;
    std::fs::rename(&retained, &private)?;

    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o755))?;
    assert!(restart().is_err());
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700))?;
    assert!(file.load()? == Some(next));
    app.logout()?;
    assert!(app.client.session().is_none() && file.load()?.is_none());
    assert!(restart()?.client.session().is_none());
    Ok(())
}
