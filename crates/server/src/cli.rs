//! Commands for the site owner, run on the server itself (never over HTTP):
//!
//! ```text
//! outhouse-server evidence-files <report-id> [--any]
//! outhouse-server evidence-cat <report-id> <storage-key>
//! outhouse-server export-evidence <report-id> [--operator NAME]
//! outhouse-server evidence-handover-log <report-id> <package-sha256> <file-count> <package-name> [--operator NAME]
//! ```
//!
//! deploy/evidence-handover.sh uses them (through `docker compose exec`) to
//! package a report's files for police without anyone opening them. They
//! work on quarantined files too: those are never served over HTTP, and this
//! only ever streams a file's bytes, unchanged, to whoever runs the command
//! on the server. Output goes to stdout; messages and logs go to stderr.

use std::io::Write;

use anyhow::{bail, Context};
use serde_json::json;

use crate::api::safety;
use crate::state::AppState;

pub const COMMANDS: &[&str] = &[
    "evidence-files",
    "evidence-cat",
    "export-evidence",
    "evidence-handover-log",
];

/// Exit codes the script understands.
pub const EXIT_NOT_FOUND: i32 = 4;
pub const EXIT_NOT_ELIGIBLE: i32 = 3;
pub const EXIT_USAGE: i32 = 2;

fn usage() -> i32 {
    eprintln!(
        "usage:\n  outhouse-server evidence-files <report-id> [--any]\n  outhouse-server evidence-cat <report-id> <storage-key>\n  outhouse-server export-evidence <report-id> [--operator NAME]\n  outhouse-server evidence-handover-log <report-id> <package-sha256> <file-count> <package-name> [--operator NAME]"
    );
    EXIT_USAGE
}

/// `--operator NAME` (who ran it on the server), and the other arguments.
fn split_args(args: &[String]) -> (Vec<String>, Option<String>, bool) {
    let mut rest = Vec::new();
    let mut operator = None;
    let mut any = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--operator" => operator = it.next().cloned(),
            "--any" => any = true,
            _ => rest.push(a.clone()),
        }
    }
    let operator = operator
        .map(|o| {
            o.chars()
                .filter(|c| !c.is_control())
                .take(64)
                .collect::<String>()
        })
        .filter(|o| !o.trim().is_empty());
    (rest, operator, any)
}

fn parse_id(raw: Option<&String>) -> Option<i64> {
    raw.and_then(|r| r.trim().parse::<i64>().ok())
        .filter(|id| *id > 0)
}

fn safe_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 128 && key.bytes().all(|b| b.is_ascii_alphanumeric())
}

pub async fn run(st: AppState, command: &str, args: &[String]) -> anyhow::Result<i32> {
    let (rest, operator, any) = split_args(args);
    let Some(id) = parse_id(rest.first()) else {
        return Ok(usage());
    };
    let report: Option<(String,)> = sqlx::query_as("SELECT category FROM reports WHERE id = $1")
        .bind(id)
        .fetch_optional(&st.db)
        .await?;
    let Some((category,)) = report else {
        eprintln!("There's no report {id}.");
        return Ok(EXIT_NOT_FOUND);
    };
    let operator_label = operator.clone().unwrap_or_else(|| "unknown".to_string());
    match command {
        "evidence-files" => {
            let rows: Vec<(String, String, String, i64, Option<String>, bool)> = sqlx::query_as(
                "SELECT f.storage_key, f.filename, f.content_type, f.size, f.sha256,
                        EXISTS(SELECT 1 FROM quarantined_files q WHERE q.storage_key = f.storage_key)
                 FROM report_files f WHERE f.report_id = $1 ORDER BY f.filename, f.storage_key",
            )
            .bind(id)
            .fetch_all(&st.db)
            .await?;
            let quarantined = rows.iter().any(|r| r.5);
            if category != safety::CHILD_SAFETY && !quarantined && !any {
                eprintln!(
                    "Report {id} isn't a child-safety report and has no quarantined files. Add --any to package it anyway."
                );
                return Ok(EXIT_NOT_ELIGIBLE);
            }
            let mut out = std::io::stdout().lock();
            // key, sha256 (as recorded when reported), size, on disk, quarantined
            for (key, _name, _type, size, sha, q) in rows {
                if !safe_key(&key) {
                    continue;
                }
                let on_disk = st.config.upload_dir.join(&key).is_file();
                writeln!(
                    out,
                    "{key}\t{}\t{size}\t{}\t{}",
                    sha.unwrap_or_else(|| "-".into()),
                    if on_disk { "yes" } else { "no" },
                    if q { "yes" } else { "no" }
                )?;
            }
            Ok(0)
        }
        "evidence-cat" => {
            let Some(key) = rest.get(1).filter(|k| safe_key(k)) else {
                return Ok(usage());
            };
            let belongs: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM report_files WHERE report_id = $1 AND storage_key = $2)",
            )
            .bind(id)
            .bind(key)
            .fetch_one(&st.db)
            .await?;
            if !belongs {
                eprintln!("That file isn't part of report {id}.");
                return Ok(EXIT_NOT_FOUND);
            }
            let path = st.config.upload_dir.join(key);
            let mut file = std::fs::File::open(&path)
                .with_context(|| format!("{key} isn't on the server any more"))?;
            let mut out = std::io::stdout().lock();
            std::io::copy(&mut file, &mut out)?;
            out.flush()?;
            Ok(0)
        }
        "export-evidence" => {
            let package = safety::evidence_package(
                &st,
                id,
                json!({ "operator": operator_label, "via": "the server console (outhouse-server export-evidence)" }),
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            let mut out = std::io::stdout().lock();
            serde_json::to_writer_pretty(&mut out, &package)?;
            writeln!(out)?;
            out.flush()?;
            safety::log_action(
                &st,
                None,
                "export",
                Some(id),
                None,
                format!(
                    "Exported the evidence package on the server (no files), by {operator_label}"
                ),
            )
            .await;
            Ok(0)
        }
        "evidence-handover-log" => {
            let (Some(hash), Some(count), Some(name)) = (rest.get(1), rest.get(2), rest.get(3))
            else {
                return Ok(usage());
            };
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!("the package SHA-256 should be 64 hex characters");
            }
            let count: u32 = count.parse().context("the file count should be a number")?;
            let name: String = name.chars().filter(|c| !c.is_control()).take(200).collect();
            safety::log_action(
                &st,
                None,
                "handover",
                Some(id),
                None,
                format!(
                    "Evidence packaged for police on the server by {operator_label}: {count} file(s) and the evidence package in {name} (SHA-256 {})",
                    hash.to_lowercase()
                ),
            )
            .await;
            eprintln!("Written to the safety log.");
            Ok(0)
        }
        _ => Ok(usage()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_and_keys() {
        let args: Vec<String> = ["12", "--operator", "kim\u{7}", "--any"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (rest, op, any) = split_args(&args);
        assert_eq!(rest, vec!["12".to_string()]);
        assert_eq!(op.as_deref(), Some("kim"));
        assert!(any);
        assert_eq!(parse_id(rest.first()), Some(12));
        assert_eq!(parse_id(Some(&"-3".to_string())), None);
        assert!(safe_key("abc123DEF"));
        assert!(!safe_key("../etc/passwd"));
        assert!(!safe_key(""));
    }
}
