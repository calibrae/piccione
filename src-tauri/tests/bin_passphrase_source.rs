//! Source-level guard: pin that the three bins delegate passphrase
//! resolution to the lib helper instead of re-implementing a `.db_key`
//! read. The original bug was three private copies of
//! `get_or_create_db_passphrase` that diverged from the keychain-aware
//! version once `.db_key.bak` started showing up on disk. This test
//! traps regressions at compile-time-of-tests.

const PAIR_ONCE: &str = include_str!("../src/bin/pair_once.rs");
const IS_PAIRED: &str = include_str!("../src/bin/is_paired.rs");
const LIST_DEVICES: &str = include_str!("../src/bin/list_devices.rs");

const BINS: &[(&str, &str)] = &[
    ("pair_once", PAIR_ONCE),
    ("is_paired", IS_PAIRED),
    ("list_devices", LIST_DEVICES),
];

#[test]
fn bins_import_lib_keychain_helper() {
    for (name, src) in BINS {
        assert!(
            src.contains("signalui_lib::store::keychain::resolve_db_passphrase_for_cli"),
            "{name} must import the lib keychain helper"
        );
    }
}

#[test]
fn bins_do_not_define_local_get_or_create_db_passphrase() {
    for (name, src) in BINS {
        assert!(
            !src.contains("fn get_or_create_db_passphrase("),
            "{name} must not redefine get_or_create_db_passphrase locally; \
             use signalui_lib::store::keychain::get_or_create_db_passphrase"
        );
    }
}

#[test]
fn bins_do_not_read_db_key_file_directly() {
    // The legacy code read `.db_key` via `read_to_string(&key_file)` and
    // wrote it via `fs::write(&key_file, …)`. Both are forbidden in bins now.
    for (name, src) in BINS {
        assert!(
            !src.contains("read_to_string(&key_file)"),
            "{name} must not read the .db_key file directly"
        );
        assert!(
            !src.contains("fs::write(&key_file"),
            "{name} must not write the .db_key file directly"
        );
    }
}
