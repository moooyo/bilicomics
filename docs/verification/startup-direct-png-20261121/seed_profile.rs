use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{BufReader, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Arc, atomic::{AtomicBool, Ordering}},
};

use anyhow::{Context, Result, ensure};
use bilicomics::source::{
    AuthSession, BiliClient, SessionFile, SourceError, crypto::NativeCrypto,
    transport::{HttpRequest, HttpResponse, ResponseFuture, Transport},
};
use image::{ColorType, ExtendedColorType, ImageDecoder, ImageEncoder, codecs::png::{PngDecoder, PngEncoder}};
use rusqlite::{Connection, OpenFlags};

const ROOT: &str = "/work/direct-png-fixture";
const MARKER: &[u8] = b"BILICOMICS DIRECT PNG OFFLINE FIXTURE V1\n";
const ENCODED: &[u8] = b"20 static 320x416 straight RGBA8 cache PNGs\n";
const ACCOUNT: &str = "bili_424242";
const WIDTH: u32 = 320;
const HEIGHT: u32 = 416;

fn private_object(path: &Path, directory: bool, owner: u32) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(meta.uid() == owner && meta.mode() & 0o077 == 0 && !meta.is_symlink()
        && if directory { meta.is_dir() } else { meta.is_file() && meta.nlink() == 1 }, "Not a private fixture object");
    Ok(())
}

fn absent(path: &Path) -> bool {
    fs::symlink_metadata(path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

fn fixture() -> Result<(PathBuf, u32)> {
    ensure!(std::env::current_dir()? == Path::new("/work"), "Run at the private worker mount");
    let interfaces = fs::read_to_string("/proc/net/dev")?;
    let names: Vec<_> = interfaces.lines().skip(2).filter_map(|line| line.split_once(':').map(|pair| pair.0.trim())).collect();
    ensure!(names == ["lo"], "Use an isolated loopback-only network namespace");
    let root = Path::new(ROOT);
    let owner = fs::metadata("/proc/self")?.uid();
    let data = root.join("data");
    for path in [root, data.as_path(), &data.join("covers")] { private_object(path, true, owner)?; }
    ensure!(root.canonicalize()? == root, "Unexpected fixture mount path");
    private_object(&root.join(".fixture-only"), false, owner)?;
    ensure!(fs::read(root.join(".fixture-only"))? == MARKER, "Missing fixture admission");
    ensure!(absent(&data.join("account")), "Refusing an existing account directory");
    for suffix in ["-wal", "-shm", "-journal"] {
        ensure!(absent(&data.join(format!("state.sqlite3{suffix}"))), "Expected a closed input database");
    }
    private_object(&data.join("state.sqlite3"), false, owner)?;
    let db = Connection::open_with_flags(format!("file:{ROOT}/data/state.sqlite3?mode=ro&immutable=1"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI)?;
    let version: u32 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let count: u32 = db.query_row("SELECT count(*) FROM comics", [], |row| row.get(0))?;
    let samples: u32 = db.query_row("SELECT count(*) FROM comics WHERE account_key=?1
        AND comic_id BETWEEN '710001' AND '710020' AND length(comic_id)=6 AND comic_id=printf('%d',CAST(comic_id AS INTEGER))
        AND json_extract(payload,'$.title')=printf('Archive Study %02d',CAST(comic_id AS INTEGER)-710000)
        AND json_extract(payload,'$.cover_url')='https://covers.fixture.invalid/studies/' || comic_id || '.png'", [ACCOUNT], |row| row.get(0))?;
    ensure!((version, count, samples) == (4, 20, 20), "Expected the closed synthetic catalog only");
    Ok((data, owner))
}

fn decode(path: &Path, owner: u32) -> Result<Vec<u8>> {
    private_object(path, false, owner)?;
    let input = File::open(path)?;
    ensure!(input.metadata()?.len() <= 1024 * 1024, "Excessive fixture PNG size");
    let decoder = PngDecoder::new(BufReader::new(input))?;
    ensure!(decoder.dimensions() == (WIDTH, HEIGHT) && decoder.color_type() == ColorType::Rgba8,
        "Expected a straight RGBA8 fixture cover");
    let mut pixels = vec![0; (WIDTH * HEIGHT * 4) as usize];
    decoder.read_image(&mut pixels)?;
    Ok(pixels)
}

fn marker_id(pixels: &[u8]) -> Result<u8> {
    let pixel = |column: usize, row: usize| { let at = (row * WIDTH as usize + column) * 4; &pixels[at..at + 4] };
    ensure!(pixel(160, 100) == [45, 170, 114, 112], "Missing translucent fixture marker");
    let mut value = 0;
    for column in [48, 104, 160, 216, 272] {
        let rgba = pixel(column, 208);
        ensure!(rgba == [32, 32, 32, 255] || rgba == [224, 224, 224, 255], "Invalid fixture marker");
        value = (value << 1) | u8::from(rgba[0] == 224);
    }
    ensure!((1..=20).contains(&value), "Unexpected fixture cover identity");
    Ok(value)
}

fn encode(data: &Path, owner: u32) -> Result<()> {
    let root = Path::new(ROOT);
    let receipt = root.join(".encoded");
    ensure!(absent(&receipt), "Refusing to overwrite an encoded seed");
    let mut paths: Vec<_> = fs::read_dir(data.join("covers"))?.map(|entry| entry.map(|entry| entry.path())).collect::<std::io::Result<_>>()?;
    ensure!(paths.len() == 20, "Expected exactly 20 fixture covers");
    paths.sort();
    let mut ids = BTreeSet::new();
    let mut before = 0;
    let mut after = 0;
    for path in paths {
        let name = path.file_stem().and_then(|part| part.to_str()).context("Invalid cache name")?;
        ensure!(name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && path.extension().is_some_and(|extension| extension == "png"), "Unexpected cache name");
        let pixels = decode(&path, owner)?;
        ensure!(ids.insert(marker_id(&pixels)?), "Duplicate fixture cover identity");
        let pending = root.join(".encoding");
        let mut output = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&pending)?;
        PngEncoder::new(&mut output).write_image(&pixels, WIDTH, HEIGHT, ExtendedColorType::Rgba8)?;
        output.sync_all()?;
        drop(output);
        ensure!(decode(&pending, owner)? == pixels, "Encoded fixture pixels changed");
        before += fs::metadata(&path)?.len();
        after += fs::metadata(&pending)?.len();
        fs::rename(pending, path)?;
    }
    OpenOptions::new().create_new(true).write(true).mode(0o600).open(receipt)?.write_all(ENCODED)?;
    println!("production_png_encoded=20 all_rgba_equal=true source_bytes={before} encoded_bytes={after}");
    Ok(())
}

struct FixtureIdentity(AtomicBool);

impl Transport for FixtureIdentity {
    fn send(&self, request: HttpRequest) -> ResponseFuture<'_> {
        Box::pin(async move {
            if request.method != "GET" || request.url != "https://api.bilibili.com/x/web-interface/nav"
                || !request.body.is_empty() || self.0.swap(true, Ordering::SeqCst) {
                return Err(SourceError::protocol("unexpected_fixture_request"));
            }
            Ok(HttpResponse { status: 200, headers: vec![], body: br#"{"code":0,"data":{"isLogin":true,"mid":424242,"uname":"Offline Fixture"}}"#.to_vec() })
        })
    }
}

fn identity(data: &Path, owner: u32) -> Result<()> {
    let receipt = Path::new(ROOT).join(".encoded");
    private_object(&receipt, false, owner)?;
    ensure!(fs::read(receipt)? == ENCODED, "Expected the completed fixture encoding");
    let transport = Arc::new(FixtureIdentity(AtomicBool::new(false)));
    let material = AuthSession::from_cookie_text("SESSDATA=fixture-only-not-a-real-session; bili_jct=fixture-only-csrf")?;
    ensure!(material.account_key().is_none(), "The unvalidated fixture must not have an account");
    let client = BiliClient::with_adapters(Some(material), data, transport.clone(), Arc::new(NativeCrypto::new(data)));
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let verified = runtime.block_on(client.validate_session())?;
    let session = client.session().context("Validation returned no session")?;
    ensure!(verified.id == "424242" && verified.name == "Offline Fixture" && session.account_key() == Some(ACCOUNT)
        && transport.0.load(Ordering::SeqCst), "Incomplete fixture validation");
    let storage = SessionFile::new(&data.join("account"));
    storage.save(&session)?;
    private_object(&storage.path(), false, owner)?;
    let restored = storage.load()?.context("Synthetic identity was not restored")?;
    ensure!(restored.account_key() == Some(ACCOUNT) && restored.account_name() == "Offline Fixture", "Unexpected restored identity");
    println!("offline_fixture_identity=validated_persisted_reloaded in_process_requests=1");
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    ensure!(args.len() == 1, "Expected encode or identity; no paths or materials are accepted");
    let (data, owner) = fixture()?;
    match args[0].to_str() {
        Some("encode") => encode(&data, owner),
        Some("identity") => identity(&data, owner),
        _ => anyhow::bail!("Expected encode or identity"),
    }
}
