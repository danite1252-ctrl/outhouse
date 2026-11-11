//! The police hand-over package: one sealed `.tar.gz` with a report's
//! original files (byte for byte, quarantined ones included), the evidence
//! package JSON, HANDOVER.txt and MANIFEST.sha256.
//!
//! One code path for every route: the safety console's one-time police
//! download link and "Download here" (api/police.rs), and the server
//! command `outhouse-server evidence-package` that deploy/evidence-handover.sh
//! uses over SSH (cli.rs).
//!
//! Nothing here opens, decodes or thumbnails a file: each one is copied
//! straight from the upload folder into the archive while its SHA-256 is
//! worked out. The package is written to the packages folder in the data
//! volume (`<UPLOAD_DIR>/.evidence-packages`, never served over HTTP) with
//! owner-only permissions.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::api::safety;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// One of a report's files, as recorded when it was reported.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReportFile {
    pub storage_key: String,
    /// The SHA-256 recorded when it was reported (if it was on disk then).
    pub sha256: Option<String>,
    pub size: i64,
    pub quarantined: bool,
}

/// Storage keys are plain letters and digits; anything else is skipped.
pub fn safe_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 128 && key.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The report's files (sorted by storage key).
pub async fn report_files(st: &AppState, report_id: i64) -> AppResult<Vec<ReportFile>> {
    Ok(sqlx::query_as::<_, ReportFile>(
        "SELECT f.storage_key, f.sha256, f.size,
                EXISTS(SELECT 1 FROM quarantined_files q WHERE q.storage_key = f.storage_key) AS quarantined
         FROM report_files f WHERE f.report_id = $1 ORDER BY f.storage_key",
    )
    .bind(report_id)
    .fetch_all(&st.db)
    .await?)
}

/// How the package was made (HANDOVER.txt, the JSON and the safety log say so).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// A one-time download link for police (the app's recommended route).
    Link,
    /// Downloaded straight to the owner's browser ("Download here").
    Browser,
    /// `outhouse-server evidence-package` on the server (evidence-handover.sh).
    Console,
}

impl Route {
    pub fn label(self) -> &'static str {
        match self {
            Route::Link => "one-time police download link (Outhouse safety console)",
            Route::Browser => {
                "downloaded in the browser by the site owner (Outhouse safety console)"
            }
            Route::Console => {
                "on the server (deploy/evidence-handover.sh / outhouse-server evidence-package)"
            }
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Route::Link => "link",
            Route::Browser => "browser",
            Route::Console => "console",
        }
    }
}

pub struct Request {
    pub report_id: i64,
    /// Who made it ("Dan (@dan), site owner" / the SSH user).
    pub operator: String,
    /// Officer name, police reference or case number (free text).
    pub reference: String,
    pub route: Route,
    /// Extra details for the JSON's `generated_by` (user id, username).
    pub generated_by: Value,
}

#[derive(Debug)]
pub struct Package {
    /// Where it is on disk (in the packages folder).
    pub path: PathBuf,
    /// `evidence-<report>-<UTC stamp>` (the folder inside, and the download name without .tar.gz).
    pub name: String,
    pub sha256: String,
    pub size: u64,
    pub file_count: usize,
    /// Storage keys no longer on the server.
    pub missing: Vec<String>,
    /// Storage keys whose bytes don't match the SHA-256 recorded when reported.
    pub mismatched: Vec<String>,
}

impl Package {
    pub fn file_name(&self) -> String {
        format!("{}.tar.gz", self.name)
    }
}

/// A package's file name in the packages folder (nothing that could leave it).
pub fn safe_package_file(name: &str) -> bool {
    name.starts_with("evidence-")
        && name.ends_with(".tar.gz")
        && name.len() <= 120
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

/// The folder packages are built in (in the data volume, never served).
pub fn packages_dir(st: &AppState) -> PathBuf {
    st.config.upload_dir.join(".evidence-packages")
}

async fn ensure_dir(dir: &Path) -> AppResult<()> {
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("creating {}: {e}", dir.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).await;
    }
    Ok(())
}

/// "2026-11-12 03:04:05 AEDT (UTC+11:00), Australia/Sydney" (Postgres knows the time zones).
async fn sydney_time(st: &AppState, at: DateTime<Utc>) -> String {
    let result: Result<String, sqlx::Error> = async {
        let mut tx = st.db.begin().await?;
        sqlx::query("SET LOCAL TIME ZONE 'Australia/Sydney'")
            .execute(&mut *tx)
            .await?;
        let s: String = sqlx::query_scalar(
            "SELECT to_char($1::timestamptz, 'YYYY-MM-DD HH24:MI:SS TZ \"(UTC\"OF\")\"')",
        )
        .bind(at)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(s)
    }
    .await;
    match result {
        Ok(s) => format!("{s}, Australia/Sydney"),
        Err(_) => "(Sydney time unavailable)".into(),
    }
}

fn clean_line(text: &str, max: usize) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Builds the package for `req` in `dir` (created if needed). The caller
/// owns the file afterwards (serves it, keeps it for a link, or deletes it).
pub async fn build(st: &AppState, req: &Request, dir: &Path) -> AppResult<Package> {
    let report_id = req.report_id;
    // The report must exist (evidence_package says NotFound otherwise).
    let files = report_files(st, report_id).await?;
    let generated_at = Utc::now();
    let mut generated_by = req.generated_by.clone();
    if let Some(obj) = generated_by.as_object_mut() {
        obj.insert("operator".into(), Value::String(req.operator.clone()));
        obj.insert("reference".into(), Value::String(req.reference.clone()));
        obj.insert("route".into(), Value::String(req.route.key().into()));
        obj.insert("via".into(), Value::String(req.route.label().into()));
    }
    let package = safety::evidence_package(st, report_id, generated_by, true).await?;
    let json = serde_json::to_vec_pretty(&package).map_err(|e| AppError::Other(e.into()))?;
    let sydney = sydney_time(st, generated_at).await;
    let server = std::env::var("PUBLIC_URL").ok().filter(|s| !s.is_empty());

    ensure_dir(dir).await?;
    let stamp = generated_at.format("%Y%m%dT%H%M%SZ").to_string();
    let name = format!("evidence-{report_id}-{stamp}");
    // A unique file on disk (two packages in the same second can't clash).
    let disk_name = format!("{name}-{}.tar.gz", &crate::auth::new_token()[..12]);
    let path = dir.join(&disk_name);

    let header = HandoverHeader {
        report_id,
        generated_utc: generated_at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        generated_sydney: sydney,
        operator: clean_line(&req.operator, 200),
        reference: clean_line(&req.reference, 200),
        route: req.route.label().to_string(),
        server,
    };
    let upload_dir = st.config.upload_dir.clone();
    let job_name = name.clone();
    let job_path = path.clone();
    let result = tokio::task::spawn_blocking(move || {
        write_package(&job_path, &job_name, &upload_dir, &files, &json, &header)
    })
    .await
    .map_err(|e| AppError::Other(anyhow::anyhow!("packing the evidence: {e}")))?;
    let written = match result {
        Ok(w) => w,
        Err(e) => {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(AppError::Other(anyhow::anyhow!(
                "packing the evidence: {e}"
            )));
        }
    };
    Ok(Package {
        path,
        name,
        sha256: written.sha256,
        size: written.size,
        file_count: written.file_count,
        missing: written.missing,
        mismatched: written.mismatched,
    })
}

struct HandoverHeader {
    report_id: i64,
    generated_utc: String,
    generated_sydney: String,
    operator: String,
    reference: String,
    route: String,
    server: Option<String>,
}

struct Written {
    sha256: String,
    size: u64,
    file_count: usize,
    missing: Vec<String>,
    mismatched: Vec<String>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Reads through to the archive, working out the SHA-256 on the way.
struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
}

/// Writes to the package file, working out its SHA-256 and size on the way.
struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
    size: u64,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        self.size += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn new_header(kind: tar::EntryType, mode: u32, size: u64, mtime: u64) -> tar::Header {
    let mut h = tar::Header::new_ustar();
    h.set_entry_type(kind);
    h.set_mode(mode);
    h.set_size(size);
    h.set_mtime(mtime);
    h.set_uid(0);
    h.set_gid(0);
    h
}

fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

fn write_package(
    path: &Path,
    name: &str,
    upload_dir: &Path,
    files: &[ReportFile],
    json: &[u8],
    h: &HandoverHeader,
) -> anyhow::Result<Written> {
    let out = create_private(path)?;
    let writer = HashingWriter {
        inner: std::io::BufWriter::new(out),
        hasher: Sha256::new(),
        size: 0,
    };
    let gz = flate2::write::GzEncoder::new(writer, flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    let mtime = Utc::now().timestamp().max(0) as u64;

    for dir in [name.to_string(), format!("{name}/files")] {
        let mut hd = new_header(tar::EntryType::Directory, 0o700, 0, mtime);
        tar.append_data(&mut hd, format!("{dir}/"), std::io::empty())?;
    }

    let mut manifest = String::new();
    let mut missing = Vec::new();
    let mut mismatched = Vec::new();
    let mut count = 0usize;
    for f in files {
        if !safe_key(&f.storage_key) {
            continue;
        }
        let src = upload_dir.join(&f.storage_key);
        let file = match std::fs::File::open(&src) {
            Ok(file) => file,
            Err(_) => {
                missing.push(f.storage_key.clone());
                continue;
            }
        };
        let len = file.metadata()?.len();
        let mut reader = HashingReader {
            // Exactly the length in the header, whatever happens to the file.
            inner: file.take(len),
            hasher: Sha256::new(),
        };
        let mut hd = new_header(tar::EntryType::Regular, 0o600, len, mtime);
        tar.append_data(
            &mut hd,
            format!("{name}/files/{}", f.storage_key),
            &mut reader,
        )?;
        let got = hex(&reader.hasher.finalize());
        if f.sha256
            .as_deref()
            .is_some_and(|want| !want.eq_ignore_ascii_case(&got))
        {
            mismatched.push(f.storage_key.clone());
        }
        manifest.push_str(&format!("{got}  files/{}\n", f.storage_key));
        count += 1;
    }

    let json_name = format!("report-{}.json", h.report_id);
    let mut hd = new_header(tar::EntryType::Regular, 0o600, json.len() as u64, mtime);
    tar.append_data(&mut hd, format!("{name}/{json_name}"), json)?;
    manifest.push_str(&format!("{}  {json_name}\n", hex(&Sha256::digest(json))));

    let text = handover_text(h, count, &missing, &mismatched, &json_name);
    let mut hd = new_header(tar::EntryType::Regular, 0o600, text.len() as u64, mtime);
    tar.append_data(&mut hd, format!("{name}/HANDOVER.txt"), text.as_bytes())?;
    manifest.push_str(&format!(
        "{}  HANDOVER.txt\n",
        hex(&Sha256::digest(text.as_bytes()))
    ));

    let mut hd = new_header(tar::EntryType::Regular, 0o600, manifest.len() as u64, mtime);
    tar.append_data(
        &mut hd,
        format!("{name}/MANIFEST.sha256"),
        manifest.as_bytes(),
    )?;

    let gz = tar.into_inner()?;
    let mut writer = gz.finish()?;
    writer.flush()?;
    let HashingWriter {
        inner,
        hasher,
        size,
    } = writer;
    let file = inner.into_inner().map_err(|e| e.into_error())?;
    file.sync_all()?;
    Ok(Written {
        sha256: hex(&hasher.finalize()),
        size,
        file_count: count,
        missing,
        mismatched,
    })
}

fn handover_text(
    h: &HandoverHeader,
    count: usize,
    missing: &[String],
    mismatched: &[String],
    json_name: &str,
) -> String {
    let mut t = String::new();
    let mut line = |s: &str| {
        t.push_str(s);
        t.push('\n');
    };
    line("Outhouse evidence hand-over");
    line("===========================");
    line(&format!("Report:        {}", h.report_id));
    line(&format!("Generated:     {}", h.generated_utc));
    line(&format!("               {}", h.generated_sydney));
    line(&format!("Operator:      {}", h.operator));
    line(&format!(
        "Officer / ref: {}",
        if h.reference.is_empty() {
            "(none given)"
        } else {
            &h.reference
        }
    ));
    line(&format!("Made:          {}", h.route));
    if let Some(server) = &h.server {
        line(&format!("Server:        {server}"));
    }
    line(&format!(
        "Files:         {count} (in files/, named by their storage key; see {json_name}"
    ));
    line("               for each file's original name, type, size, the SHA-256 recorded");
    line("               when it was reported and the automatic image check's result)");
    if !missing.is_empty() {
        line(&format!(
            "Not included:  {} file(s) no longer on the server: {}",
            missing.len(),
            missing.join(" ")
        ));
    }
    if !mismatched.is_empty() {
        line(&format!(
            "WARNING:       {} file(s) don't match the SHA-256 recorded when reported: {}",
            mismatched.len(),
            mismatched.join(" ")
        ));
    }
    line("");
    line("The files were copied byte for byte from the Outhouse server's upload");
    line("store. They were not opened, viewed, decoded or converted.");
    line("");
    line(&format!(
        "{json_name} is everything Outhouse knows about the report:"
    ));
    line("the report and what was reported, who reported it, the reported account");
    line("(with its suspension state), the IP addresses it used (UTC and Sydney");
    line("time), attempts from blocked networks and the safety log.");
    line("");
    line("The SHA-256 of this whole package (the .tar.gz) can't be written inside");
    line("it. Outhouse showed it when the package was made and recorded it in its");
    line("safety log (\"Evidence packaged for police\"); check the file you received");
    line("with:   sha256sum <package>.tar.gz");
    line("");
    line("To check nothing inside has changed, extract it and in this folder run:");
    line("    sha256sum -c MANIFEST.sha256");
    line("");
    line("Do not open these files.");
    t
}

/// The safety log line for a package.
pub fn log_details(req: &Request, p: &Package, extra: &str) -> String {
    let reference = clean_line(&req.reference, 200);
    let mut s = format!(
        "Evidence packaged for police ({}) by {}: {} file(s) and the evidence package in {} ({} bytes), SHA-256 {}",
        match req.route {
            Route::Link => "one-time download link",
            Route::Browser => "downloaded here, in the browser",
            Route::Console => "on the server",
        },
        clean_line(&req.operator, 200),
        p.file_count,
        p.file_name(),
        p.size,
        p.sha256
    );
    if !reference.is_empty() {
        s.push_str(&format!(". Officer / reference: {reference}"));
    }
    if !p.missing.is_empty() {
        s.push_str(&format!(
            ". {} file(s) no longer on the server",
            p.missing.len()
        ));
    }
    if !p.mismatched.is_empty() {
        s.push_str(&format!(
            ". WARNING: {} file(s) don't match their recorded SHA-256",
            p.mismatched.len()
        ));
    }
    s.push_str(extra);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_round_trip() {
        let dir = std::env::temp_dir().join(format!("oh-handover-{}", crate::auth::new_token()));
        let uploads = dir.join("uploads");
        std::fs::create_dir_all(&uploads).unwrap();
        let bytes: Vec<u8> = (0..70_000u32).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(uploads.join("abc123"), &bytes).unwrap();
        let want = hex(&Sha256::digest(&bytes));
        let files = vec![
            ReportFile {
                storage_key: "abc123".into(),
                sha256: Some(want.clone()),
                size: 70_000,
                quarantined: true,
            },
            ReportFile {
                storage_key: "gone1".into(),
                sha256: None,
                size: 1,
                quarantined: false,
            },
            ReportFile {
                storage_key: "../etc".into(),
                sha256: None,
                size: 1,
                quarantined: false,
            },
        ];
        let h = HandoverHeader {
            report_id: 7,
            generated_utc: "2026-01-01 00:00:00 UTC".into(),
            generated_sydney: "x".into(),
            operator: "kim".into(),
            reference: "Det. Smith 123".into(),
            route: Route::Link.label().into(),
            server: None,
        };
        let out = dir.join("p.tar.gz");
        let w = write_package(&out, "evidence-7-x", &uploads, &files, b"{}", &h).unwrap();
        assert_eq!(w.file_count, 1);
        assert_eq!(w.missing, vec!["gone1".to_string()]);
        assert!(w.mismatched.is_empty());
        let raw = std::fs::read(&out).unwrap();
        assert_eq!(w.sha256, hex(&Sha256::digest(&raw)));
        assert_eq!(w.size, raw.len() as u64);
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(&raw[..]));
        let mut seen = std::collections::HashMap::new();
        for e in ar.entries().unwrap() {
            let mut e = e.unwrap();
            let p = e.path().unwrap().to_string_lossy().to_string();
            let mut b = Vec::new();
            e.read_to_end(&mut b).unwrap();
            seen.insert(p, b);
        }
        assert_eq!(seen["evidence-7-x/files/abc123"], bytes);
        let manifest = String::from_utf8(seen["evidence-7-x/MANIFEST.sha256"].clone()).unwrap();
        assert!(manifest.contains(&format!("{want}  files/abc123")));
        assert!(manifest.contains("  report-7.json"));
        let text = String::from_utf8(seen["evidence-7-x/HANDOVER.txt"].clone()).unwrap();
        assert!(text.contains("Det. Smith 123") && text.contains("Do not open these files."));
        assert!(text.contains("no longer on the server: gone1"));
        assert!(safe_package_file(
            "evidence-7-20260101T000000Z-0123456789ab.tar.gz"
        ));
        assert!(!safe_package_file("evidence-../x.tar.gz"));
        assert!(!safe_package_file("evidence-7/x.tar.gz"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
