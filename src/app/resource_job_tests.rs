//! Headless application scheduling tests; no Slint window or live service is used.

use std::{
    future::{Future, poll_fn},
    sync::{Mutex, atomic::{AtomicUsize, Ordering}},
    task::Poll,
};

use tokio::sync::oneshot;

use super::*;

pub(super) async fn tick_until(app: &mut App, mut finished: impl FnMut(&App) -> bool) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            app.tick()?;
            if finished(app) { return Ok::<(), anyhow::Error>(()); }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.context("The application callback did not finish before the test deadline")?
}

struct CallbackDropped(Option<oneshot::Sender<()>>);

impl Drop for CallbackDropped {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() { let _ = sender.send(()); }
    }
}

#[tokio::test]
async fn protected_backend_persists_without_waiting_for_paused_ui_delivery() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let client = BiliClient::offline(directory.path())?;
    let mut app = test_support::app(directory.path(), DeviceProfile::Paperwhite5,
        client, tokio::runtime::Handle::current())?;
    let mut held_tickets = Vec::new();
    let mut held_delivery = Vec::new();
    for sequence in 0..8 {
        let ticket = app.jobs.begin(app.epoch, sequence, &format!("resource-paused-ui-{sequence}"), false)?;
        held_delivery.push(ticket.acquire_delivery().await.context("The fixture could not occupy UI delivery")?);
        held_tickets.push(ticket);
    }
    let database = app.store.path().to_owned();
    let account = app.account.clone();
    let (persisted, observed) = oneshot::channel();
    let callbacks = Arc::new(AtomicUsize::new(0));
    let completed = callbacks.clone();
    app.source_job("purchase-submit:resource-ui-backpressure", async move {
        tokio::task::spawn_blocking(move || -> Result<()> {
            Store::open(database)?.set_preference(&account, "resource_receipt_before_ui", &"stored")
        }).await.map_err(|_| source::SourceError::storage("resource_test_worker_failed"))?
            .map_err(|_| source::SourceError::storage("resource_test_storage_failed"))?;
        let _ = persisted.send(());
        Ok(())
    }, move |_, result| {
        assert!(result.is_ok());
        completed.fetch_add(1, Ordering::SeqCst);
    });
    tokio::time::timeout(Duration::from_secs(5), observed).await
        .context("Paused UI delivery delayed the protected backend")??;
    assert_eq!(app.store.preference::<String>(&app.account, "resource_receipt_before_ui")?, Some("stored".into()));
    assert_eq!(callbacks.load(Ordering::SeqCst), 0);
    drop(held_delivery);
    tick_until(&mut app, |_| callbacks.load(Ordering::SeqCst) == 1).await?;
    drop(held_tickets);
    Ok(())
}

#[tokio::test]
async fn callback_requeue_preserves_the_new_pending_generation_until_its_own_result() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let client = BiliClient::offline(directory.path())?;
    let mut app = test_support::app(directory.path(), DeviceProfile::Paperwhite5,
        client, tokio::runtime::Handle::current())?;
    let callbacks = Arc::new(Mutex::new(Vec::new()));
    let first_callbacks = callbacks.clone();
    let (release_second, second_released) = oneshot::channel();
    app.source_job("resource-requeue", async { Ok(1_u8) }, move |app, result| {
        first_callbacks.lock().unwrap().push(result.unwrap());
        let second_callbacks = first_callbacks.clone();
        app.source_job("resource-requeue", async move {
            second_released.await.map_err(|_| source::SourceError::capability("resource_test_gate_closed"))?;
            Ok(2_u8)
        }, move |_, result| second_callbacks.lock().unwrap().push(result.unwrap()));
    });
    let first_sequence = app.pending["resource-requeue"];
    tick_until(&mut app, |_| callbacks.lock().unwrap().len() == 1).await?;
    let second_sequence = *app.pending.get("resource-requeue")
        .context("The first callback removed the replacement job's pending generation")?;
    assert!(second_sequence > first_sequence);
    assert_eq!(callbacks.lock().unwrap().as_slice(), &[1]);
    release_second.send(()).map_err(|_| anyhow::anyhow!("The replacement source future was cancelled"))?;
    tick_until(&mut app, |_| callbacks.lock().unwrap().len() == 2).await?;
    assert_eq!(callbacks.lock().unwrap().as_slice(), &[1, 2]);
    assert!(!app.pending.contains_key("resource-requeue"));
    Ok(())
}

async fn panicking_source() -> source::Result<()> { panic!("Controlled application source panic") }

#[tokio::test]
async fn a_panicking_source_future_delivers_one_typed_error_and_clears_its_pending_key() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let client = BiliClient::offline(directory.path())?;
    let mut app = test_support::app(directory.path(), DeviceProfile::Paperwhite5,
        client, tokio::runtime::Handle::current())?;
    let observed = Arc::new(Mutex::new(None));
    let calls = Arc::new(AtomicUsize::new(0));
    let callback_observed = observed.clone();
    let callback_calls = calls.clone();
    app.source_job("resource-source-panic", panicking_source(), move |_, result| {
        *callback_observed.lock().unwrap() = Some(result.expect_err("A panicking source must fail"));
        callback_calls.fetch_add(1, Ordering::SeqCst);
    });
    tick_until(&mut app, |_| calls.load(Ordering::SeqCst) == 1).await?;
    let observed = observed.lock().unwrap();
    let error = observed.as_ref().context("The source panic did not produce an application error")?;
    assert_eq!(error.kind, ErrorKind::Unavailable);
    assert_eq!(error.code, "application_source_worker_failed");
    assert_eq!(error.transmitted, None);
    assert!(!app.pending.contains_key("resource-source-panic"));
    drop(observed);
    app.tick()?;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn full_regular_admission_preserves_the_download_manager_for_recover_and_remove() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let client = BiliClient::offline(directory.path())?;
    let mut app = test_support::app(directory.path(), DeviceProfile::Paperwhite5,
        client.clone(), tokio::runtime::Handle::current())?;
    app.options.offline = true;
    app.downloads = Some(DownloadManager::new(directory.path(), app.account.clone(), app.epoch,
        Arc::new(client), tokio::runtime::Handle::current())?);
    let held = (0..128).map(|index| app.begin_job(&format!("resource-capacity-{index}"), false))
        .collect::<Result<Vec<_>>>()?;
    let sequence_before = app.sequence;
    app.recover_downloads();
    assert!(app.downloads.is_some());
    assert!(!app.pending.contains_key("download-recover"));
    app.download_action("open", "resource-comic")?;
    app.download_action("remove-current", "")?;
    assert!(!app.pending.contains_key("download-remove"));
    assert_eq!(app.sequence, sequence_before);
    let manager = app.downloads.as_mut().context("Capacity rejection discarded the download manager")?;
    manager.set_concurrency(2)?;
    manager.pause_all()?;
    drop(held);
    Ok(())
}

#[tokio::test]
async fn durable_source_finishes_local_persistence_after_account_clear_without_applying_old_ui() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let client = BiliClient::offline(directory.path())?;
    let mut app = test_support::app(directory.path(), DeviceProfile::Paperwhite5,
        client, tokio::runtime::Handle::current())?;
    let database = app.store.path().to_owned();
    let previous_account = app.account.clone();
    let write_account = previous_account.clone();
    let (started, did_start) = oneshot::channel();
    let (release_backend, backend_released) = oneshot::channel();
    let (discarded, did_discard) = oneshot::channel();
    let callback_drop = CallbackDropped(Some(discarded));
    let callback_calls = Arc::new(AtomicUsize::new(0));
    let recorded_calls = callback_calls.clone();
    // This local receipt fixture exercises scheduling, not a financial service.
    app.source_job("purchase-submit:resource-local-receipt", async move {
        let _ = started.send(());
        backend_released.await.map_err(|_| source::SourceError::capability("resource_test_gate_closed"))?;
        tokio::task::spawn_blocking(move || -> Result<()> {
            let store = Store::open(database)?;
            store.set_preference(&write_account, "resource_observed_receipt", &"persisted")?;
            Ok(())
        }).await.map_err(|_| source::SourceError::storage("resource_test_worker_failed"))?
            .map_err(|_| source::SourceError::storage("resource_test_storage_failed"))?;
        Ok(())
    }, move |_, _| {
        recorded_calls.fetch_add(1, Ordering::SeqCst);
        drop(callback_drop);
    });
    tokio::time::timeout(Duration::from_secs(5), did_start).await
        .context("The durable backend did not start")??;
    app.jobs.clear();
    app.pending.clear();
    app.epoch += 1;
    app.account = "bili_22".into();
    release_backend.send(()).map_err(|_| anyhow::anyhow!("UI invalidation cancelled durable execution"))?;
    tokio::time::timeout(Duration::from_secs(5), did_discard).await
        .context("The stale durable UI callback was not discarded")??;
    assert_eq!(app.store.preference::<String>(&previous_account, "resource_observed_receipt")?, Some("persisted".into()));
    assert!(app.store.preference::<String>(&app.account, "resource_observed_receipt")?.is_none());
    app.tick()?;
    assert_eq!(callback_calls.load(Ordering::SeqCst), 0);
    assert!(!app.pending.contains_key("purchase-submit:resource-local-receipt"));
    Ok(())
}

#[tokio::test]
async fn cancelling_background_waiting_does_not_release_the_running_workers_delivery_slot() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let client = BiliClient::offline(directory.path())?;
    let mut app = test_support::app(directory.path(), DeviceProfile::Paperwhite5,
        client, tokio::runtime::Handle::current())?;
    let mut held_tickets = Vec::new();
    let mut held_delivery = Vec::new();
    for sequence in 0..7 {
        let ticket = app.jobs.begin(app.epoch, sequence, &format!("resource-held-{sequence}"), false)?;
        held_delivery.push(ticket.acquire_delivery().await.context("A fixture delivery slot was unavailable")?);
        held_tickets.push(ticket);
    }
    let (started, did_start) = oneshot::channel();
    let (release_worker, worker_released) = std::sync::mpsc::channel();
    let (discarded, did_discard) = oneshot::channel();
    let callback_drop = CallbackDropped(Some(discarded));
    let callback_calls = Arc::new(AtomicUsize::new(0));
    let recorded_calls = callback_calls.clone();
    app.background("resource-held-worker", move || {
        let _ = started.send(());
        worker_released.recv().context("The blocking fixture's release channel closed")?;
        Ok(())
    }, move |_, _| {
        recorded_calls.fetch_add(1, Ordering::SeqCst);
        drop(callback_drop);
    });
    tokio::time::timeout(Duration::from_secs(5), did_start).await
        .context("The real blocking worker did not start")??;
    app.invalidate("resource-held-worker");
    tokio::time::timeout(Duration::from_secs(5), did_discard).await
        .context("The cancelled background awaiter did not finish")??;
    let next = app.jobs.begin(app.epoch, 99, "resource-after-cancel", false)?;
    let mut delivery = Box::pin(next.acquire_delivery());
    let pending = poll_fn(|context| Poll::Ready(Future::poll(delivery.as_mut(), context).is_pending())).await;
    assert!(pending, "Cancelling the awaiter released the running worker's result slot");
    release_worker.send(()).context("The blocking worker exited before being released")?;
    let permit = tokio::time::timeout(Duration::from_secs(5), delivery).await
        .context("The finished blocking worker retained its result slot")?
        .context("The replacement job was unexpectedly cancelled")?;
    assert_eq!(callback_calls.load(Ordering::SeqCst), 0);
    assert!(!app.pending.contains_key("resource-held-worker"));
    drop(permit);
    drop(held_delivery);
    drop(held_tickets);
    Ok(())
}
