//! Read-only persisted page candidates. Metadata describes the last-opened page,
//! not proof that the document is still on screen. Continuous input ownership and
//! an established editing contract remain mandatory before `supported` is set.
use crate::workflow::history::{Owner, PageState};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const META_LIMIT: usize = 1024 * 1024;

fn bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "Native metadata/page exceeds limit");
    Ok(bytes)
}

fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

fn latest(entries: &[(String, Value)]) -> Result<(String, usize)> {
    let mut chosen = None;
    let mut time = 0;
    let mut ambiguous = false;
    for (id, value) in entries {
        if value["type"] != "DocumentType" || value["deleted"] == true || value["parent"] == "trash"
        {
            continue;
        }
        let Some(opened) = value["lastOpened"].as_str() else {
            continue;
        };
        let opened: u64 = opened.parse().context("Invalid native last-opened time")?;
        if opened == 0 {
            continue;
        }
        ensure!(uuid(id), "Invalid native document ID");
        if opened > time {
            time = opened;
            chosen = Some((
                id.clone(),
                usize::try_from(
                    value["lastOpenedPage"]
                        .as_u64()
                        .context("Missing last-opened page index")?,
                )?,
            ));
            ambiguous = false;
        } else if opened == time {
            ambiguous = true;
        }
    }
    ensure!(!ambiguous, "Ambiguous last-opened document");
    chosen.context("No last-opened document")
}

fn documents(root: &Path) -> Result<Vec<(String, Value)>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "metadata") {
            continue;
        }
        ensure!(entries.len() < 4096, "Native document inventory limit");
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .context("Invalid metadata path")?
            .to_owned();
        let value = serde_json::from_slice(&bounded(&path, META_LIMIT)?)?;
        entries.push((id, value));
    }
    Ok(entries)
}

fn page_identity(content: &Value, expected_index: usize) -> Result<(String, String)> {
    let last = &content["cPages"]["lastOpened"];
    let page = last["value"].as_str().context("Missing native page ID")?;
    ensure!(uuid(page), "Invalid native page ID");
    let visit = last["timestamp"]
        .as_str()
        .context("Missing native visit revision")?;
    let (author, revision) = visit
        .split_once(':')
        .context("Invalid native visit revision")?;
    author.parse::<u64>()?;
    revision.parse::<u64>()?;
    let pages = content["cPages"]["pages"]
        .as_array()
        .context("Missing native page order")?;
    ensure!(
        !pages.is_empty() && pages.len() <= 10000,
        "Native page count limit"
    );
    let mut order = Vec::new();
    for entry in pages {
        if entry["deleted"]["value"] == true {
            continue;
        }
        let id = entry["id"].as_str().context("Missing ordered page ID")?;
        let idx = entry["idx"]["value"]
            .as_str()
            .context("Missing page ordering key")?;
        ensure!(
            uuid(id) && !idx.is_empty() && idx.len() <= 256,
            "Invalid page ordering entry"
        );
        order.push((idx, id));
    }
    order.sort_unstable();
    ensure!(
        !order.windows(2).any(|p| p[0].0 == p[1].0),
        "Ambiguous page ordering keys"
    );
    ensure!(
        order.iter().filter(|(_, id)| *id == page).count() == 1
            && order.get(expected_index).is_some_and(|(_, id)| *id == page),
        "Page metadata is inconsistent or not settled"
    );
    Ok((page.into(), visit.into()))
}

pub fn candidate(root: &Path, session: String) -> Result<PageState> {
    let selected = latest(&documents(root)?)?;
    let result = load_selected(root, session, &selected)?;
    ensure!(
        selected == latest(&documents(root)?)?,
        "Last-opened metadata changed during observation"
    );
    Ok(result)
}

fn parse_last_open(settings: &str) -> Result<Option<String>> {
    let mut section = "";
    let mut found = None;
    for line in settings.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
        }
        if !matches!(section, "" | "General") {
            continue;
        }
        if let Some(value) = line.strip_prefix("LastOpen=") {
            ensure!(found.is_none(), "Duplicate LastOpen setting");
            let value = value
                .strip_prefix("@ByteArray(")
                .and_then(|v| v.strip_suffix(')'))
                .context("Unsupported LastOpen encoding")?;
            ensure!(
                value.is_empty() || uuid(value),
                "Invalid LastOpen document ID"
            );
            found = Some((!value.is_empty()).then(|| value.to_owned()));
        }
    }
    found.context("Missing LastOpen setting")
}

fn last_open(settings: &Path) -> Result<Option<String>> {
    let bytes = bounded(settings, META_LIMIT)?;
    parse_last_open(std::str::from_utf8(&bytes)?)
}

pub fn observed_owner(root: &Path, settings: &Path, session: String) -> Result<Owner> {
    let document = last_open(settings)?.context("No open document")?;
    let metadata: Value = serde_json::from_slice(&bounded(
        &root.join(format!("{document}.metadata")),
        META_LIMIT,
    )?)?;
    let content: Value = serde_json::from_slice(&bounded(
        &root.join(format!("{document}.content")),
        META_LIMIT,
    )?)?;
    let index = usize::try_from(
        metadata["lastOpenedPage"]
            .as_u64()
            .context("Missing page index")?,
    )?;
    let (page, visit) = page_identity(&content, index)?;
    ensure!(
        last_open(settings)?.as_ref() == Some(&document),
        "Open document changed during observation"
    );
    Ok(Owner {
        document,
        page,
        visit,
        session,
    })
}

pub fn verified_contract(model: super::DeviceModel, release: &str) -> bool {
    let versions: Vec<_> = release
        .lines()
        .filter_map(|line| line.strip_prefix("IMG_VERSION="))
        .collect();
    model == super::DeviceModel::Remarkable2
        && versions.len() == 1
        && versions[0].trim().trim_matches('"') == "3.28.0.172"
}

pub fn session_matches(proc_root: &Path, session: &str) -> Result<bool> {
    let (pid, expected) = session
        .split_once(':')
        .context("Invalid recorded native session")?;
    ensure!(
        !pid.is_empty() && pid.bytes().all(|c| c.is_ascii_digit()),
        "Invalid recorded PID"
    );
    let expected: u64 = expected.parse()?;
    let path = proc_root.join(pid);
    ensure!(
        bounded(&path.join("comm"), 256)? == b"xochitl\n",
        "Native process identity changed"
    );
    let stat = String::from_utf8(bounded(&path.join("stat"), 16384)?)?;
    let fields = stat
        .rsplit_once(')')
        .context("Invalid native process stat")?
        .1;
    let start: u64 = fields
        .split_whitespace()
        .nth(19)
        .context("Missing native start ticks")?
        .parse()?;
    Ok(start == expected)
}

/// LastOpen was observed clearing in the library and following document opens.
/// It persists in the page overview, so input observation is still essential.
/// Never infer editing permission from this read-only result alone.
pub fn observed_candidate(root: &Path, settings: &Path, session: String) -> Result<PageState> {
    let document = last_open(settings)?.context("No open document")?;
    let metadata: Value = serde_json::from_slice(&bounded(
        &root.join(format!("{document}.metadata")),
        META_LIMIT,
    )?)?;
    let index = usize::try_from(
        metadata["lastOpenedPage"]
            .as_u64()
            .context("Missing page index")?,
    )?;
    let result = load_selected(root, session, &(document.clone(), index))?;
    ensure!(
        last_open(settings)?.as_ref() == Some(&document),
        "Open document changed during observation"
    );
    Ok(result)
}

fn load_selected(root: &Path, session: String, selected: &(String, usize)) -> Result<PageState> {
    ensure!(!session.is_empty(), "Missing native process identity");
    let meta_path = root.join(format!("{}.metadata", selected.0));
    let content_path = root.join(format!("{}.content", selected.0));
    let metadata = bounded(&meta_path, META_LIMIT)?;
    let content = bounded(&content_path, META_LIMIT)?;
    let identity = page_identity(&serde_json::from_slice(&content)?, selected.1)?;
    let path = root.join(&selected.0).join(format!("{}.rm", identity.0));
    let native = bounded(&path, 16 * 1024 * 1024)?;
    let text = super::native_text::read(&native)?;
    ensure!(
        metadata == bounded(&meta_path, META_LIMIT)?
            && content == bounded(&content_path, META_LIMIT)?
            && native == bounded(&path, 16 * 1024 * 1024)?,
        "Native files changed during observation"
    );
    let mut seal = Vec::new();
    for bytes in [metadata, content, native] {
        seal.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        seal.extend_from_slice(&bytes);
    }
    Ok(PageState {
        owner: Owner {
            document: selected.0.clone(),
            page: identity.0,
            visit: identity.1,
            session,
        },
        content: text,
        seal,
        supported: false,
    })
}

/// PID plus kernel start ticks detects a restart even if a PID is reused.
pub fn xochitl_session(proc_root: &Path) -> Result<String> {
    let mut found = Vec::new();
    for entry in fs::read_dir(proc_root)? {
        let path: PathBuf = entry?.path();
        let Some(pid) = path
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
        else {
            continue;
        };
        let Ok(comm) = bounded(&path.join("comm"), 256) else {
            continue;
        };
        if comm != b"xochitl\n" {
            continue;
        }
        let stat = String::from_utf8(bounded(&path.join("stat"), 16384)?)?;
        let fields = stat
            .rsplit_once(')')
            .context("Invalid native process stat")?
            .1;
        let start: u64 = fields
            .split_whitespace()
            .nth(19)
            .context("Missing native process start ticks")?
            .parse()?;
        found.push(format!("{pid}:{start}"));
    }
    ensure!(
        found.len() == 1,
        "Native process identity is unavailable or ambiguous"
    );
    Ok(found.remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const DOC: &str = "00000000-0000-0000-0000-000000000001";
    const PAGE: &str = "00000000-0000-0000-0000-000000000002";
    #[test]
    fn metadata_ties_and_unsafe_paths_are_not_guessed() {
        let meta = json!({"type":"DocumentType", "lastOpened":"123", "lastOpenedPage":0});
        assert!(latest(&[(DOC.into(), meta.clone()), (PAGE.into(), meta.clone())]).is_err());
        assert!(latest(&[("../escape".into(), meta.clone())]).is_err());
        assert_eq!(latest(&[(DOC.into(), meta)]).unwrap(), (DOC.into(), 0));
    }
    #[test]
    fn last_opened_index_and_revision_must_agree_with_ordered_page_identity() {
        let mut content = json!({"cPages":{"lastOpened":{"value":PAGE,"timestamp":"1:4"},"pages":[{"id":PAGE,"idx":{"value":"ba"}}]}});
        assert_eq!(
            page_identity(&content, 0).unwrap(),
            (PAGE.into(), "1:4".into())
        );
        assert!(page_identity(&content, 1).is_err());
        content["cPages"]["lastOpened"]["timestamp"] = json!("1:6");
        assert_eq!(page_identity(&content, 0).unwrap().1, "1:6");
        content["cPages"]["lastOpened"]["value"] = json!("../escape");
        assert!(page_identity(&content, 0).is_err());
    }

    #[test]
    fn last_open_only_reads_the_document_setting_and_rejects_unknown_encodings() {
        let settings = format!(
            "[General]\nUserToken=ignored\nLastOpen=@ByteArray({DOC})\n[Other]\nLastOpen=ignored\n"
        );
        assert_eq!(parse_last_open(&settings).unwrap(), Some(DOC.into()));
        assert_eq!(
            parse_last_open("[General]\nLastOpen=@ByteArray()\n").unwrap(),
            None
        );
        assert!(parse_last_open("LastOpen=../escape").is_err());
        assert!(parse_last_open("LastOpen=@ByteArray()\nLastOpen=@ByteArray()").is_err());
        assert!(parse_last_open("[Other]\nLastOpen=@ByteArray()").is_err());
    }

    #[test]
    fn native_edit_contract_does_not_silently_enable_untested_firmware() {
        use super::super::DeviceModel;
        assert!(verified_contract(
            DeviceModel::Remarkable2,
            "IMG_VERSION=\"3.28.0.172\"\n"
        ));
        assert!(!verified_contract(
            DeviceModel::Remarkable2,
            "IMG_VERSION=\"3.29.0.1\"\n"
        ));
        assert!(!verified_contract(
            DeviceModel::RemarkablePaperPro,
            "IMG_VERSION=\"3.28.0.172\"\n"
        ));
        assert!(!verified_contract(DeviceModel::Remarkable2, ""));
    }
}
