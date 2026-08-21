//! Regression guard for `.github/workflows/release-sandbox.yml`.
//!
//! libkrun v1.19.4's Makefile declares `install: libkrun.pc` — the install
//! target does *not* depend on `all`, so `make install` alone never builds
//! the shared library and the workflow step fails with
//! `install: cannot stat 'target/release/libkrun.so.1.19.4'`. The workflow
//! must run `make` (the `all` target) before `make install`.
//!
//! The step must also provide libclang: libkrun's own cargo build runs
//! bindgen (libkrunfw-sys), and dirge's default-features build runs bindgen
//! too (janetrs). clang-sys panics when no `libclang.so` is available.

use std::fs;

fn workflow() -> String {
    fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/.github/workflows/release-sandbox.yml"
    ))
    .expect("read release-sandbox.yml")
}

/// Body of the `Build libkrun from source` step: everything between its
/// `- name:` header and the next step's.
fn libkrun_step(workflow: &str) -> &str {
    workflow
        .split("- name: Build libkrun from source")
        .nth(1)
        .expect("workflow has a libkrun build step")
        .split("\n- name:")
        .next()
        .expect("libkrun build step body")
}

/// Body of the step named `name`: everything between its `- name:` header
/// and the next step's.
fn step<'a>(workflow: &'a str, name: &str) -> &'a str {
    let header = format!("- name: {name}");
    workflow
        .split(&header)
        .nth(1)
        .unwrap_or_else(|| panic!("workflow has a `{name}` step"))
        .split("\n- name:")
        .next()
        .unwrap_or_else(|| panic!("`{name}` step body"))
}

#[test]
fn libkrun_step_builds_before_install() {
    let workflow = workflow();
    let step = libkrun_step(&workflow);

    let build = step
        .lines()
        .position(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("make ") && !trimmed.starts_with("make install")
        })
        .expect("workflow must run `make` (libkrun `all` target)");
    let install = step
        .lines()
        .position(|line| line.trim_start().starts_with("make install"))
        .expect("workflow must run `make install`");

    assert!(
        build < install,
        "`make` must run before `make install`: libkrun's install target \
         does not depend on `all`, so it never builds the shared library"
    );
}

#[test]
fn libkrun_step_provides_libclang_for_bindgen() {
    let workflow = workflow();
    let step = libkrun_step(&workflow);

    let provides = step.contains("libclang-dev") || step.contains("LIBCLANG_PATH");
    assert!(
        provides,
        "libkrun's cargo build (and dirge's default features) run bindgen; \
         without libclang-dev or LIBCLANG_PATH, clang-sys fails with \
         `couldn't find any valid shared libraries matching: ['libclang.so', ...]`"
    );
}

/// The `ds-sandbox` tag is overwritten on every push to `deepseek`, so
/// nix/bin-sandbox.nix must track the hash of whatever was just uploaded;
/// otherwise Nix fetches a stale artifact for the new tag contents.
#[test]
fn workflow_refreshes_bin_sandbox_hash_after_upload() {
    let workflow = workflow();

    let upload = workflow
        .find("- name: Upload release assets")
        .expect("workflow has an upload step");
    let update = workflow
        .find("- name: Update nix/bin-sandbox.nix")
        .expect("workflow has an update step for nix/bin-sandbox.nix");
    assert!(upload < update, "nix update must run after the upload");

    let step = step(&workflow, "Update nix/bin-sandbox.nix");
    assert!(
        step.contains("nix/bin-sandbox.nix"),
        "update step must rewrite nix/bin-sandbox.nix"
    );
    assert!(
        step.contains("sha256-"),
        "update step must emit an SRI `sha256-...` hash (Nix fixed-output format)"
    );
    assert!(
        step.contains("re.sub") || step.contains("sed"),
        "update step must replace the hash in place"
    );
}

/// The hash bump must be committed back to `deepseek` so the repo tracks
/// the uploaded artifact. A GITHUB_TOKEN push doesn't re-trigger workflows,
/// so the commit can't loop this workflow.
#[test]
fn workflow_commits_bin_sandbox_bump_to_deepseek() {
    let workflow = workflow();

    let update = workflow
        .find("- name: Update nix/bin-sandbox.nix")
        .expect("workflow has an update step for nix/bin-sandbox.nix");
    let commit = workflow
        .find("- name: Commit nix/bin-sandbox.nix bump")
        .expect("workflow has a commit step for nix/bin-sandbox.nix");
    assert!(update < commit, "commit must run after the update");

    let step = step(&workflow, "Commit nix/bin-sandbox.nix bump");
    assert!(
        step.contains("git push origin"),
        "commit step must push the bump"
    );
    assert!(
        step.contains("deepseek"),
        "commit step must push to the `deepseek` branch"
    );
}
