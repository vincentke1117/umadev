//! Spec test-vector runner — implements UMADEV_HOST_SPEC_V1 §1.4.
//!
//! Reads `tests/spec_vectors/<clause>.json` and asserts that the implementation
//! named by each clause produces the expected result for every vector. Content
//! rules use governance decisions; UD-CODE-003 uses the typed contract validator
//! because the specification explicitly defines API alignment as a post-write
//! contract check rather than a pre-write content block.
//!
//! The vectors live at the workspace root (`tests/spec_vectors/`) so they're
//! shared across crates; this test reads them via a relative path.

use serde::Deserialize;
use umadev_contract::{
    validate::ViolationKind, ApiSpec, Endpoint, FrontendCall, HttpVerb, SecurityKind,
};
use umadev_governance::{check_ai_slop, check_color_tokens, check_emoji, Decision};

/// One `(file_path, content) → expected_decision` vector.
#[derive(Debug, Deserialize)]
struct Vector {
    file_path: String,
    content: String,
    expected_decision: String,
}

/// The vector file shape (see tests/spec_vectors/UD-CODE-001.json).
#[derive(Debug, Deserialize)]
struct VectorFile {
    clause: String,
    vectors: Vec<Vector>,
}

/// One endpoint or frontend call in the UD-CODE-003 contract vectors.
#[derive(Debug, Deserialize)]
struct ContractTuple {
    method: String,
    path: String,
}

/// One API-alignment vector from `UD-CODE-003.json`.
#[derive(Debug, Deserialize)]
struct ContractVector {
    name: String,
    contract_endpoints: Vec<ContractTuple>,
    frontend_calls: Vec<ContractTuple>,
    expected_violations: usize,
    #[serde(default)]
    violation_kind: Option<String>,
}

/// The distinct vector-file shape used by the typed contract clause.
#[derive(Debug, Deserialize)]
struct ContractVectorFile {
    clause: String,
    vectors: Vec<ContractVector>,
}

/// Resolve the workspace-root spec_vectors dir. The test binary runs from
/// the crate dir, so we walk up to find `tests/spec_vectors`.
fn spec_vectors_dir() -> std::path::PathBuf {
    let mut dir = std::env::current_dir().unwrap();
    for _ in 0..6 {
        let candidate = dir.join("tests/spec_vectors");
        if candidate.is_dir() {
            return candidate;
        }
        dir = match dir.parent() {
            Some(p) => p.to_path_buf(),
            None => break,
        };
    }
    // Fall back to the workspace layout (crate is at crates/<name>, vectors at top).
    std::path::PathBuf::from("../../tests/spec_vectors")
}

fn load_vectors(clause: &str) -> Vec<Vector> {
    let path = spec_vectors_dir().join(format!("{clause}.json"));
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let parsed: VectorFile =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("malformed {}: {e}", path.display()));
    assert_eq!(
        parsed.clause, clause,
        "clause field mismatch in {clause}.json"
    );
    parsed.vectors
}

fn load_contract_vectors() -> Vec<ContractVector> {
    let clause = "UD-CODE-003";
    let path = spec_vectors_dir().join(format!("{clause}.json"));
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let parsed: ContractVectorFile =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("malformed {}: {e}", path.display()));
    assert_eq!(
        parsed.clause, clause,
        "clause field mismatch in {clause}.json"
    );
    parsed.vectors
}

fn parse_http_verb(method: &str, vector_name: &str) -> HttpVerb {
    HttpVerb::parse(method)
        .unwrap_or_else(|| panic!("UD-CODE-003 vector `{vector_name}` has invalid verb `{method}`"))
}

fn assert_decision(actual: &Decision, expected: &str, clause: &str, file_path: &str) {
    let actual_blocked = actual.block;
    let want_block = expected == "block";
    assert!(
        actual_blocked == want_block,
        "{} vector for `{}`: expected decision `{expected}` but got `{}`\n  reason: {}",
        clause,
        file_path,
        if actual_blocked { "block" } else { "pass" },
        actual.reason,
    );
}

#[test]
fn sd_code_001_emoji_vectors_pass() {
    for v in load_vectors("UD-CODE-001") {
        let d = check_emoji(&v.file_path, &v.content);
        assert_decision(&d, &v.expected_decision, "UD-CODE-001", &v.file_path);
    }
}

#[test]
fn sd_code_002_color_vectors_pass() {
    for v in load_vectors("UD-CODE-002") {
        let d = check_color_tokens(&v.file_path, &v.content);
        assert_decision(&d, &v.expected_decision, "UD-CODE-002", &v.file_path);
    }
}

#[test]
fn ud_code_003_api_alignment_vectors_pass() {
    for vector in load_contract_vectors() {
        let spec = ApiSpec {
            title: format!("fixture: {}", vector.name),
            endpoints: vector
                .contract_endpoints
                .iter()
                .enumerate()
                .map(|(index, endpoint)| Endpoint {
                    method: parse_http_verb(&endpoint.method, &vector.name),
                    path: endpoint.path.clone(),
                    operation_id: format!("fixtureOperation{index}"),
                    description: String::new(),
                    request_shape: String::new(),
                    response_shape: String::new(),
                    security: SecurityKind::None,
                })
                .collect(),
        };
        let calls: Vec<FrontendCall> = vector
            .frontend_calls
            .iter()
            .map(|call| FrontendCall {
                file: "fixture.ts".to_string(),
                method: parse_http_verb(&call.method, &vector.name),
                path: call.path.clone(),
                method_known: true,
            })
            .collect();

        let violations = umadev_contract::validate_frontend_vs_contract(&calls, &spec);
        assert_eq!(
            violations.len(),
            vector.expected_violations,
            "UD-CODE-003 vector `{}` produced unexpected violations: {violations:?}",
            vector.name
        );
        if let Some(expected) = vector.violation_kind.as_deref() {
            let expected = match expected {
                "UndeclaredCall" => ViolationKind::UndeclaredCall,
                "MethodMismatch" => ViolationKind::MethodMismatch,
                other => panic!(
                    "UD-CODE-003 vector `{}` has unknown violation kind `{other}`",
                    vector.name
                ),
            };
            assert!(
                violations
                    .iter()
                    .all(|violation| violation.kind == expected),
                "UD-CODE-003 vector `{}` expected {expected:?}, got {violations:?}",
                vector.name
            );
        }
    }
}

#[test]
fn sd_code_005_slop_vectors_pass() {
    for v in load_vectors("UD-CODE-005") {
        let d = check_ai_slop(&v.file_path, &v.content);
        assert_decision(&d, &v.expected_decision, "UD-CODE-005", &v.file_path);
    }
}
