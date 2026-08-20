//! Regression guard for `.github/workflows/release-sandbox.yml`.
//!
//! libkrun v1.19.4's Makefile declares `install: libkrun.pc` — the install
//! target does *not* depend on `all`, so `make install` alone never builds
//! the shared library and the workflow step fails with
//! `install: cannot stat 'target/release/libkrun.so.1.19.4'`. The workflow
//! must run `make` (the `all` target) before `make install`.

use std::fs;

#[test]
fn libkrun_step_builds_before_install() {
    let workflow = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/.github/workflows/release-sandbox.yml"
    ))
    .expect("read release-sandbox.yml");

    let step = workflow
        .split("- name: Build libkrun from source")
        .nth(1)
        .expect("workflow has a libkrun build step")
        .split("\n- name:")
        .next()
        .expect("libkrun build step body");

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
