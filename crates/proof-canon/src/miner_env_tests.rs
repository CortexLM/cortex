//! Miner BYOK env: shape, allowlist, and the no-leak contract of [`MinerEnv`].

use std::collections::BTreeMap;

use super::*;

const KEY: &str = "MINER_PROVIDED_API_KEY";
const OTHER: &str = "MINER_PROVIDED_BASE_URL";

fn constraints(params: &[(&str, &str)]) -> Constraints {
    Constraints {
        params: params
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        ..Constraints::default()
    }
}

fn env(pairs: &[(&str, &str)]) -> MinerEnv {
    let mut e = MinerEnv::new();
    for (k, v) in pairs {
        e.insert(k, v);
    }
    e
}

#[test]
fn env_names_are_upper_snake_and_never_shadow_the_guest_contract() {
    for good in [KEY, "A", "A1", "SOME_TOKEN", &"A".repeat(MAX_ENV_NAME_LEN)] {
        assert!(is_env_name(good), "{good:?}");
    }
    for bad in [
        "",
        "lower_case",
        "1LEADING_DIGIT",
        "_LEADING_UNDERSCORE",
        "HAS-DASH",
        "HAS SPACE",
        "PROOF_PARAM_X",
        "PROOF_SECRETS_DIR",
        "PATH",
        "HOME",
        "LANG",
        "XDG_RUNTIME_DIR",
        &"A".repeat(MAX_ENV_NAME_LEN + 1),
    ] {
        assert!(!is_env_name(bad), "{bad:?} must not be a miner env name");
    }
    // The reserved prefix is the guest's own contract, whatever follows it.
    assert!(RESERVED_ENV_PREFIX.starts_with("PROOF"));
    assert!(!is_env_name(&format!("{RESERVED_ENV_PREFIX}ANYTHING")));
}

/// A topic that declares nothing takes nothing: the default is not "allow".
#[test]
fn a_topic_that_declares_no_byok_accepts_no_miner_env() {
    let silent = Constraints::default();
    assert!(silent.miner_env_allowlist().is_empty());
    assert!(silent.miner_env_required().is_empty());
    assert!(!silent.inject_miner_env_sister());
    MinerEnv::new()
        .accept(&silent)
        .expect("an empty body is fine");
    let err = env(&[(KEY, "sk-value")])
        .accept(&silent)
        .expect_err("nothing is declared");
    assert_eq!(
        err,
        MinerEnvError::Undeclared {
            name: KEY.into(),
            allowed: "(none)".into(),
        }
    );
    assert!(err.to_string().contains("(none)"), "{err}");
}

/// `miner_byok` alone is a one-element allowlist **and** a requirement — the
/// live shape, so a topic needs no reseal to take a miner key.
#[test]
fn miner_byok_is_a_single_element_allowlist_and_a_requirement() {
    let c = constraints(&[(PARAM_MINER_BYOK, KEY)]);
    assert_eq!(c.miner_env_allowlist(), vec![KEY.to_owned()]);
    assert_eq!(c.miner_env_required(), vec![KEY.to_owned()]);
    let kept = env(&[(KEY, "sk-value")]).accept(&c).expect("declared");
    assert_eq!(kept.names(), vec![KEY]);
    assert_eq!(kept.iter().collect::<Vec<_>>(), vec![(KEY, "sk-value")]);

    let err = MinerEnv::new().accept(&c).expect_err("required");
    assert_eq!(err, MinerEnvError::Missing { name: KEY.into() });
    assert!(err.to_string().contains(PARAM_MINER_BYOK), "{err}");
    assert!(
        err.to_string().contains("operator key is never used"),
        "{err}"
    );

    let err = env(&[(KEY, "sk-value"), (OTHER, "https://example.invalid")])
        .accept(&c)
        .expect_err("one name only");
    assert_eq!(
        err,
        MinerEnvError::Undeclared {
            name: OTHER.into(),
            allowed: KEY.into(),
        }
    );
}

/// The list form is optional (accepted, never demanded) and `miner_byok`
/// stays allowed on top of it.
#[test]
fn the_allowlist_widens_what_is_accepted_without_demanding_it() {
    let c = constraints(&[
        (
            PARAM_MINER_ENV_ALLOWLIST,
            &format!(" {OTHER} , {OTHER} ,, "),
        ),
        (PARAM_MINER_BYOK, KEY),
    ]);
    assert_eq!(
        c.miner_env_allowlist(),
        vec![OTHER.to_owned(), KEY.to_owned()],
        "deduplicated, byok appended"
    );
    assert_eq!(c.miner_env_required(), vec![KEY.to_owned()]);
    env(&[(KEY, "sk-value")])
        .accept(&c)
        .expect("the optional name may be omitted");
    let both = env(&[(KEY, "sk-value"), (OTHER, "https://example.invalid")])
        .accept(&c)
        .expect("both declared");
    assert_eq!(both.len(), 2);
    // Optional-only topic: nothing is demanded.
    let optional = constraints(&[(PARAM_MINER_ENV_ALLOWLIST, OTHER)]);
    assert!(optional.miner_env_required().is_empty());
    MinerEnv::new().accept(&optional).expect("nothing required");
}

#[test]
fn values_are_trimmed_bounded_and_never_blank() {
    let c = constraints(&[(PARAM_MINER_BYOK, KEY)]);
    let kept = env(&[(&format!("  {KEY}  "), "  sk-value  ")])
        .accept(&c)
        .expect("trimmed on both sides");
    assert_eq!(kept.iter().collect::<Vec<_>>(), vec![(KEY, "sk-value")]);

    for blank in ["", "   ", "\t"] {
        assert_eq!(
            env(&[(KEY, blank)]).accept(&c),
            Err(MinerEnvError::Empty { name: KEY.into() })
        );
    }
    assert_eq!(
        env(&[(KEY, &"x".repeat(MAX_MINER_ENV_VALUE_LEN + 1))]).accept(&c),
        Err(MinerEnvError::BadValue { name: KEY.into() })
    );
    assert_eq!(
        env(&[(KEY, "line\nbreak")]).accept(&c),
        Err(MinerEnvError::BadValue { name: KEY.into() })
    );
    env(&[(KEY, &"x".repeat(MAX_MINER_ENV_VALUE_LEN))])
        .accept(&c)
        .expect("exactly the cap");

    let mut many = MinerEnv::new();
    for i in 0..=MAX_MINER_ENV_VARS {
        many.insert(&format!("NAME_{i}"), "v");
    }
    assert_eq!(
        many.accept(&c),
        Err(MinerEnvError::TooMany {
            got: MAX_MINER_ENV_VARS + 1
        })
    );
}

/// A name the guest owns is refused on shape before the allowlist is even
/// consulted, so a topic cannot declare its way into overwriting the contract.
#[test]
fn a_reserved_name_is_refused_even_when_a_topic_declares_it() {
    let c = constraints(&[(PARAM_MINER_BYOK, "PROOF_SECRETS_DIR")]);
    assert!(
        c.miner_env_allowlist().is_empty(),
        "a malformed declaration never widens the list"
    );
    assert!(c.miner_env_required().is_empty());
    let err = env(&[("PROOF_SECRETS_DIR", "/tmp/evil")])
        .accept(&c)
        .expect_err("reserved");
    assert_eq!(
        err,
        MinerEnvError::BadName {
            name: "PROOF_SECRETS_DIR".into(),
        }
    );
    for name in RESERVED_ENV_NAMES {
        assert_eq!(
            env(&[(name, "x")]).accept(&c),
            Err(MinerEnvError::BadName {
                name: (*name).to_owned()
            })
        );
    }
    // And such a topic never publishes.
    assert!(c.validate_shape().is_err());
}

/// Publishing is where a malformed declaration is caught.
#[test]
fn publish_refuses_a_malformed_declaration() {
    constraints(&[(PARAM_MINER_BYOK, KEY)])
        .validate_shape()
        .expect("well formed");
    constraints(&[(PARAM_MINER_ENV_ALLOWLIST, &format!("{KEY},{OTHER}"))])
        .validate_shape()
        .expect("well formed list");
    for (key, value) in [
        (PARAM_MINER_BYOK, "lower_case"),
        (PARAM_MINER_BYOK, " , "),
        (PARAM_MINER_ENV_ALLOWLIST, "OK_NAME,not ok"),
        (PARAM_MINER_ENV_ALLOWLIST, "PROOF_JOB"),
        (PARAM_INJECT_MINER_ENV_SISTER, "yes"),
    ] {
        let err = constraints(&[(key, value)])
            .validate_shape()
            .expect_err("malformed");
        assert!(err.field.contains(key), "{err:?} should name {key}");
    }
    // An empty value never reaches the BYOK check: the generic param shape
    // refuses it first, which is the same publish reject.
    let err = constraints(&[(PARAM_MINER_BYOK, "")])
        .validate_shape()
        .expect_err("empty value");
    assert_eq!(err.field, "constraints.params");
    for (value, want) in [("true", true), ("false", false), ("TRUE", true)] {
        let c = constraints(&[(PARAM_INJECT_MINER_ENV_SISTER, value)]);
        c.validate_shape().expect("boolean");
        assert_eq!(c.inject_miner_env_sister(), want);
    }
}

/// The whole point of the newtype: a value never reaches a log line, even
/// when something formats a structure that happens to hold one.
#[test]
fn debug_prints_names_and_never_a_value() {
    let e = env(&[(KEY, "sk-super-secret-value")]);
    let printed = format!("{e:?}");
    assert!(!printed.contains("sk-super-secret-value"), "{printed}");
    assert!(printed.contains(KEY), "{printed}");
    assert!(printed.contains("[REDACTED]"), "{printed}");
    let nested = format!("{:?}", vec![e.clone()]);
    assert!(!nested.contains("sk-super-secret-value"), "{nested}");
    assert_eq!(e.secret_values(), vec![b"sk-super-secret-value".to_vec()]);

    // On the wire it is the plain object a miner posts.
    let json = serde_json::to_string(&e).expect("json");
    assert_eq!(json, format!("{{\"{KEY}\":\"sk-super-secret-value\"}}"));
    let back: MinerEnv = serde_json::from_str(&json).expect("round trip");
    assert_eq!(back, e);
    let empty: MinerEnv = serde_json::from_str("{}").expect("empty");
    assert!(empty.is_empty() && MinerEnv::new().is_empty());
    assert_eq!(
        serde_json::from_value::<BTreeMap<String, String>>(
            serde_json::to_value(&e).expect("value")
        )
        .expect("map"),
        BTreeMap::from([(KEY.to_owned(), "sk-super-secret-value".to_owned())])
    );
}
