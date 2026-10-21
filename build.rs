use std::path::{Path, PathBuf};

fn main() {
    emit_commit_hash();
}

/// Embed build provenance, including when this checkout is a linked worktree.
fn emit_commit_hash() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let commit = git_commit(&root);
    println!(
        "cargo:rustc-env=NOTEPIPE_COMMIT_HASH={}",
        commit.as_deref().unwrap_or("unknown")
    );
}

/// Resolve `HEAD` to a 12-character prefix via vergen-gix (pure Rust gix backend,
/// no git CLI dependency, handles linked worktrees). Returns `None` outside a git
/// checkout so source archives embed the fallback instead of an enclosing repo.
fn git_commit(root: &Path) -> Option<String> {
    // Source archives have no .git; do not accidentally use an enclosing repository.
    if !root.join(".git").exists() {
        return None;
    }
    let mut gix = vergen_gix::Gix::builder().sha(false).build();
    gix.at_path(root.to_path_buf());
    vergen_gix::Emitter::default()
        .quiet()
        .add_instructions(&gix)
        .ok()?
        .emit_and_set()
        .ok()?;
    // `.sha(false)` yields the full id; `--short=12` is its 12-character prefix.
    let sha = std::env::var("VERGEN_GIT_SHA").ok()?;
    let commit = sha.get(..12)?.to_owned();
    // vergen watches HEAD and the resolved ref. The reflog also changes on every
    // commit (including in linked worktrees, where the ref lives in the common
    // gitdir) and is cheap to watch.
    for reflog in head_reflogs(root) {
        if reflog.is_file() {
            println!("cargo:rerun-if-changed={}", reflog.display());
        }
    }
    Some(commit)
}

/// Reflog paths that record commit movement for this checkout: `.git/logs/HEAD`,
/// or the per-worktree reflog a linked worktree's `.git` pointer file refers to.
fn head_reflogs(root: &Path) -> Vec<PathBuf> {
    let git = root.join(".git");
    if git.is_dir() {
        return vec![git.join("logs").join("HEAD")];
    }
    // A worktree's .git is a pointer file: "gitdir: <private gitdir>".
    let pointer = std::fs::read_to_string(&git).unwrap_or_default();
    let Some(gitdir) = pointer.trim().strip_prefix("gitdir:").map(str::trim) else {
        return vec![];
    };
    let gitdir = if Path::new(gitdir).is_absolute() {
        PathBuf::from(gitdir)
    } else {
        root.join(gitdir)
    };
    vec![gitdir.join("logs").join("HEAD")]
}
