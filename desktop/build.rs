fn main() {
    emit_commit();
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/windows.rc");
        println!("cargo:rerun-if-changed=assets/icon.ico");
        // GPUI loads icon resource 1 for the window class. Embedding the same
        // resource also gives Explorer and shortcuts the application icon.
        embed_resource::compile_for(
            "assets/windows.rc",
            ["course2md-desktop"],
            embed_resource::ParamsIncludeDirs(["assets"]),
        )
        .manifest_required()
        .expect("Windows application icon must be embedded");
    }
    // The shared core can link the native Apple speech module. A library's
    // rustc-link-arg is not inherited by its consumers, so each final executable
    // must supply the system Swift runtime search path.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}

fn emit_commit() {
    use std::path::PathBuf;

    let desktop = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let root = desktop.parent().unwrap().to_path_buf();
    let commit = git_commit(&root);
    println!(
        "cargo:rustc-env=COURSE2MD_DESKTOP_COMMIT={}",
        commit.as_deref().unwrap_or("")
    );
}

/// Resolve `HEAD` to a 12-character prefix via vergen-gix (pure Rust gix backend,
/// no git CLI dependency, handles linked worktrees). Returns `None` outside a git
/// checkout so archive builds omit the row instead of attributing an enclosing
/// repository's commit to this application.
fn git_commit(root: &std::path::Path) -> Option<String> {
    use std::path::PathBuf;

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
    let git = root.join(".git");
    let reflog = if git.is_dir() {
        git.join("logs").join("HEAD")
    } else {
        // A worktree's .git is a pointer file: "gitdir: <private gitdir>".
        let pointer = std::fs::read_to_string(&git).unwrap_or_default();
        let gitdir = pointer.trim().strip_prefix("gitdir:").map(str::trim)?;
        let gitdir = if std::path::Path::new(gitdir).is_absolute() {
            PathBuf::from(gitdir)
        } else {
            root.join(gitdir)
        };
        gitdir.join("logs").join("HEAD")
    };
    if reflog.is_file() {
        println!("cargo:rerun-if-changed={}", reflog.display());
    }
    Some(commit)
}
