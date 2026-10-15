use std::env;
use std::error::Error;
use std::process::Command;
use vergen_gitcl::{Emitter, Gitcl};

fn git(args: &[&str]) -> Result<String, Box<dyn Error>> {
    let result = Command::new("git").args(args).output()?;
    if !result.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&result.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8(result.stdout)?.trim().to_owned())
}

fn semantic_tag(tag: &str) -> bool {
    let Some(version) = tag.strip_prefix('v') else {
        return false;
    };
    let parts: Vec<_> = version.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
        })
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed=READER_BUDDY_RELEASE_TAG");
    println!("cargo:rerun-if-env-changed=READER_BUDDY_RELEASE_SHA");
    println!("cargo:rerun-if-env-changed=READER_BUDDY_CANDIDATE_TAG");
    println!("cargo:rerun-if-env-changed=READER_BUDDY_CANDIDATE_SHA");
    let candidate_tag = env::var("READER_BUDDY_CANDIDATE_TAG").ok();
    let candidate_sha = env::var("READER_BUDDY_CANDIDATE_SHA").ok();
    let candidate = candidate_tag.is_some() || candidate_sha.is_some();
    if candidate
        && (env::var_os("READER_BUDDY_RELEASE_TAG").is_some()
            || env::var_os("READER_BUDDY_RELEASE_SHA").is_some())
    {
        return Err("Candidate and published-tag metadata cannot be combined".into());
    }
    let tag = candidate_tag.or_else(|| env::var("READER_BUDDY_RELEASE_TAG").ok());
    let sha = candidate_sha.or_else(|| env::var("READER_BUDDY_RELEASE_SHA").ok());
    if tag.is_some() != sha.is_some() {
        return Err("Official builds require both expected tag and SHA".into());
    }
    // Tag creation and source edits must invalidate cached development metadata.
    for name in ["HEAD", "index", "packed-refs", "refs/tags"] {
        if let Ok(path) = git(&["rev-parse", "--git-path", name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Ok(paths) = git(&["ls-files", "-z"]) {
        for path in paths.split('\0').filter(|path| !path.is_empty()) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let (Some(tag), Some(sha)) = (&tag, &sha) {
        if env::vars_os().any(|(key, _)| key.to_string_lossy().starts_with("VERGEN_")) {
            return Err("Official metadata overrides are forbidden".into());
        }
        if !semantic_tag(tag)
            || git(&["rev-parse", "--is-shallow-repository"])? != "false"
            || git(&["rev-parse", "HEAD"])? != *sha
            || (!candidate && git(&["rev-parse", &format!("refs/tags/{tag}^{{commit}}")])? != *sha)
            || !git(&["status", "--porcelain", "--untracked-files=normal"])?.is_empty()
        {
            return Err(
                "Official build requires the exact semantic tag/SHA and full clean source".into(),
            );
        }
    }
    let metadata = Gitcl::builder()
        .describe(true, true, Some("v[0-9]*"))
        .sha(false)
        .dirty(true)
        .build();
    let mut emitter = Emitter::default();
    if tag.is_some() {
        emitter.fail_on_error();
    }
    emitter.add_instructions(&metadata)?.emit_and_set()?;
    let description = env::var("VERGEN_GIT_DESCRIBE").unwrap_or_default();
    let version = if let (Some(tag), Some(sha)) = (tag, sha) {
        if (!candidate && description != tag)
            || env::var("VERGEN_GIT_SHA")? != sha
            || env::var("VERGEN_GIT_DIRTY")? != "false"
        {
            return Err("Generated Git metadata disagrees with the official tag/SHA".into());
        }
        tag[1..].to_owned()
    } else if description.is_empty() || description == "VERGEN_IDEMPOTENT_OUTPUT" {
        "dev.unknown".to_owned()
    } else {
        format!("dev.{description}")
    };
    println!("cargo:rustc-env=READER_BUDDY_VERSION={version}");
    Ok(())
}
