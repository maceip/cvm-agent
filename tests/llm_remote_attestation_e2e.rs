use ed25519_dalek::SigningKey;
use cvm_agent::eat::EatToken;
use cvm_agent::llm_attested::{
    build_capture_ra_claim, insert_capture_ra_evidence, sha256_prefixed, CaptureRaError,
    ContestEvent, ContestManifest, EventActor, TrustPolicy, CAPTURE_RA_PROFILE,
    REMOTE_ATTESTED_CAPTURE_PATHS,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

fn load_receipt(name: &str) -> (Vec<u8>, String) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("testdata/chain/{name}"));
    let cbor = fs::read(&path).unwrap();
    let eat = EatToken::from_cbor(&cbor).unwrap();
    (cbor, format!("sha384:{}", hex::encode(eat.value_x)))
}

fn manifest(receipt: &[u8], value_x: &str, key: &SigningKey) -> ContestManifest {
    let mut capture_methods = REMOTE_ATTESTED_CAPTURE_PATHS
        .iter()
        .map(|spec| spec.capture_method.to_string())
        .collect::<Vec<_>>();
    capture_methods.sort();
    capture_methods.dedup();
    ContestManifest::new(
        "https://events.example".to_string(),
        "hk_ra".to_string(),
        "gw_ra".to_string(),
        "https://events.example/e/hk_ra".to_string(),
        "https://gateway.example/.well-known/llm-attested/manifest.cbor".to_string(),
        sha256_prefixed(receipt),
        value_x.to_string(),
        "sha256:policy".to_string(),
        "sha256:scorer".to_string(),
        "routed".to_string(),
        "https://events.example/e/hk_ra/join".to_string(),
        capture_methods,
        TrustPolicy::self_hosted(
            vec!["tdx".to_string()],
            "https://gateway.example/.well-known/cvm/receipt".to_string(),
            "https://events.example/.well-known/cvm/registry.json".to_string(),
        ),
        &key.verifying_key(),
        vec!["llm.call".to_string()],
    )
}

#[test]
fn every_remote_attested_capture_path_spec_builds_claim_from_tdx_fixture() {
    let (receipt, value_x) = load_receipt("tdx_stage1.cbor");
    let key = SigningKey::from_bytes(&[10u8; 32]);
    let manifest = manifest(&receipt, &value_x, &key);
    let manifest_hash = manifest.hash().unwrap();

    for spec in REMOTE_ATTESTED_CAPTURE_PATHS {
        let claim = build_capture_ra_claim(
            &manifest,
            &manifest_hash,
            &receipt,
            spec.capture_method,
            spec.assurance,
            spec.enforcement,
        )
        .unwrap_or_else(|err| {
            panic!(
                "RA claim should build for {}/{}/{}: {err}",
                spec.capture_method, spec.assurance, spec.enforcement
            )
        });
        assert_eq!(claim.profile, CAPTURE_RA_PROFILE);
        assert_eq!(claim.capture_method, spec.capture_method);
        assert_eq!(claim.assurance, spec.assurance);
        assert_eq!(claim.enforcement, spec.enforcement);
        assert_eq!(claim.requirement, spec.requirement);
        assert_eq!(claim.platform, "tdx");
        assert_eq!(claim.value_x, value_x);
        assert_eq!(claim.cvm_receipt_hash, sha256_prefixed(&receipt));
        assert_eq!(claim.manifest_hash, manifest_hash);
        assert!(claim.chain_depth >= 2);
    }
}

#[test]
fn tdx_gateway_capture_ra_claim_verifies_from_hardware_fixture() {
    let (receipt, value_x) = load_receipt("tdx_stage1.cbor");
    let key = SigningKey::from_bytes(&[9u8; 32]);
    let manifest = manifest(&receipt, &value_x, &key);
    let manifest_hash = manifest.hash().unwrap();
    let claim = build_capture_ra_claim(
        &manifest,
        &manifest_hash,
        &receipt,
        "gateway_proxy",
        "attested",
        "routed",
    )
    .unwrap();

    assert_eq!(claim.profile, CAPTURE_RA_PROFILE);
    assert_eq!(claim.platform, "tdx");
    assert_eq!(claim.value_x, value_x);
    assert_eq!(claim.cvm_receipt_hash, sha256_prefixed(&receipt));
    assert!(
        claim.chain_depth >= 2,
        "stage 1 fixture must chain to stage 0"
    );
}

#[test]
fn tdx_tee_workspace_capture_ra_claim_verifies_from_same_contract() {
    let (receipt, value_x) = load_receipt("tdx_stage1.cbor");
    let key = SigningKey::from_bytes(&[8u8; 32]);
    let manifest = manifest(&receipt, &value_x, &key);
    let manifest_hash = manifest.hash().unwrap();
    let claim = build_capture_ra_claim(
        &manifest,
        &manifest_hash,
        &receipt,
        "tee_workspace",
        "attested",
        "full_tee",
    )
    .unwrap();

    assert_eq!(claim.capture_method, "tee_workspace");
    assert_eq!(claim.enforcement, "full_tee");
    assert_eq!(claim.platform, "tdx");
}

#[test]
fn tampered_receipt_hash_is_rejected() {
    let (receipt, value_x) = load_receipt("tdx_stage1.cbor");
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let mut manifest = manifest(&receipt, &value_x, &key);
    manifest.cvm_receipt_hash = sha256_prefixed(b"different receipt");
    let manifest_hash = manifest.hash().unwrap();
    let err = build_capture_ra_claim(
        &manifest,
        &manifest_hash,
        &receipt,
        "gateway_proxy",
        "attested",
        "routed",
    )
    .unwrap_err();
    assert!(matches!(err, CaptureRaError::ReceiptHashMismatch { .. }));
}

#[test]
fn attested_event_carries_claim_evidence_and_gateway_signature() {
    let (receipt, value_x) = load_receipt("tdx_stage1.cbor");
    let key = SigningKey::from_bytes(&[6u8; 32]);
    let manifest = manifest(&receipt, &value_x, &key);
    let manifest_hash = manifest.hash().unwrap();
    let claim = build_capture_ra_claim(
        &manifest,
        &manifest_hash,
        &receipt,
        "gateway_proxy",
        "attested",
        "routed",
    )
    .unwrap();
    let mut evidence = BTreeMap::new();
    evidence.insert("enforcement_mode".to_string(), "routed".to_string());
    insert_capture_ra_evidence(&mut evidence, &claim);
    let mut event = ContestEvent::new(
        "evt_ra".to_string(),
        "llm.call".to_string(),
        "hk_ra".to_string(),
        "team_ra".to_string(),
        "gw_ra".to_string(),
        manifest_hash,
        "gateway_proxy".to_string(),
        "attested".to_string(),
        0,
        "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        1,
        2,
        EventActor {
            kind: "agent".to_string(),
            agent_session_id: "agent_ra".to_string(),
        },
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        evidence,
    );
    event.sign(&key).unwrap();
    event
        .verify(&manifest.event_verifying_key().unwrap())
        .unwrap();
    assert_eq!(event.evidence["ra_profile"], CAPTURE_RA_PROFILE);
    assert_eq!(event.evidence["ra_platform"], "tdx");
}
