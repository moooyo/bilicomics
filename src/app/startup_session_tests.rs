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
