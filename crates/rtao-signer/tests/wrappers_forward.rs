//! Every wrapping `Signer` and `KeyView` in the workspace forwards the hashed
//! and post-quantum surface (D-171).
//!
//! A wrapper that delegates `sign` but not `authority()` turns a hashed or
//! post-quantum key into one that cannot sign at all, and a key view that
//! delegates its addresses but not its descriptors makes fee quotes price a
//! 64-byte signature and skips registration. This scans the workspace's
//! non-test sources for such wrappers.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" && name != "target" {
                sources(&path, out);
            }
        } else if name.ends_with(".rs") && !name.ends_with("tests.rs") && !name.starts_with("test_")
        {
            out.push(path);
        }
    }
}

/// The text of each `impl ... for ...` block whose header names `trait_name`,
/// with the header.
fn impl_blocks<'a>(text: &'a str, trait_name: &str) -> Vec<(usize, &'a str)> {
    let mut blocks = Vec::new();
    let mut search = 0;
    while let Some(found) = text[search..].find("impl") {
        let start = search + found;
        search = start + 4;
        let at_line_start = text[..start]
            .rsplit('\n')
            .next()
            .is_some_and(|prefix| prefix.trim().is_empty());
        if !at_line_start {
            continue;
        }
        let Some(open) = text[start..].find('{').map(|offset| start + offset) else {
            break;
        };
        let header = &text[start..open];
        let names_trait = header.split(" for ").next().is_some_and(|head| {
            head.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|word| word == trait_name)
        });
        if !header.contains(" for ") || !names_trait {
            continue;
        }
        let mut depth = 0usize;
        let mut end = open;
        for (offset, byte) in text[open..].bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + offset + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        let line = text[..start].matches('\n').count() + 1;
        blocks.push((line, &text[start..end]));
    }
    blocks
}

fn workspace_sources() -> Vec<PathBuf> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&crates).unwrap() {
        let src = entry.unwrap().path().join("src");
        if src.is_dir() {
            sources(&src, &mut files);
        }
    }
    assert!(files.len() > 100, "workspace sources not found");
    files
}

#[test]
fn wrapping_signers_forward_their_authority() {
    let mut missing = Vec::new();
    let mut wrappers = 0;
    for file in workspace_sources() {
        let text = std::fs::read_to_string(&file).unwrap();
        for (line, block) in impl_blocks(&text, "Signer") {
            let delegates = block.contains(".sign(request)");
            if delegates {
                wrappers += 1;
                if !block.contains("fn authority(") {
                    missing.push(format!("{}:{line}", file.display()));
                }
            }
        }
    }
    assert!(wrappers >= 3, "found only {wrappers} wrapping signers");
    assert!(
        missing.is_empty(),
        "wrapping signers that drop the hashed/post-quantum authority: {missing:?}"
    );
}

#[test]
fn wrapping_key_views_forward_their_descriptors() {
    let mut missing = Vec::new();
    let mut wrappers = 0;
    for file in workspace_sources() {
        let text = std::fs::read_to_string(&file).unwrap();
        for (line, block) in impl_blocks(&text, "KeyView") {
            // A wrapper delegates its addresses to another key view.
            let delegates = block.contains(".hotkey_address()")
                && block.contains(".coldkey_address()")
                && block.contains("self.");
            if !delegates {
                continue;
            }
            wrappers += 1;
            for method in [
                "fn hotkey_descriptor(",
                "fn local_descriptor(",
                "fn authority_descriptor(",
            ] {
                if !block.contains(method) {
                    missing.push(format!("{}:{line} lacks {method}", file.display()));
                }
            }
        }
    }
    assert!(wrappers >= 3, "found only {wrappers} wrapping key views");
    assert!(
        missing.is_empty(),
        "wrapping key views that drop descriptors: {missing:?}"
    );
}
