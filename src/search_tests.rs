use super::*;
use crate::{
    language::Language,
    prediction::{Answer, Usage},
    snapshot::{Change, SourceFile},
    syntax::Syntax,
};

fn fixture(lines: usize) -> Snapshot {
    let source = (1..=lines)
        .map(|line| format!("fn line_{line}() {{}}\n"))
        .collect::<String>();
    Snapshot {
        base_commit: "baseline".into(),
        fingerprint: "frozen".into(),
        language: Language::Rust,
        files: vec![SourceFile {
            path: "main.rs".into(),
            syntax: Syntax::parse(Language::Rust, "main.rs", &source).unwrap(),
            source,
            revision: "working_tree",
        }],
        changes: vec![Change {
            path: "main.rs".into(),
            diff: "a frozen diff".into(),
        }],
        context: vec![],
        warnings: vec![],
    }
}

fn options(max_calls: usize) -> Options {
    Options {
        model: "jev-latest".into(),
        top_errors: 2,
        max_calls,
        max_bytes: 200_000,
    }
}

fn answer(request: &Value, errors: &[usize]) -> Response {
    Response {
        model: "jev-pinned".into(),
        usage: Usage {
            input_tokens: 100,
            output_tokens: 10,
        },
        answers: request["questions"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, question)| {
                let instructions = &question["instructions"];
                let start = instructions["region"]["start_line"].as_u64().unwrap_or(0) as usize;
                let end = instructions["region"]["end_line"].as_u64().unwrap_or(0) as usize;
                let positive = instructions["category"]["id"] == "type_mismatch"
                    && errors.iter().any(|line| start <= *line && *line <= end);
                (
                    key.clone(),
                    Answer {
                        kind: "noul".into(),
                        noul: if positive { 0.96 } else { 0.05 },
                    },
                )
            })
            .collect(),
    }
}

#[test]
fn finds_multiple_errors_and_rechecks_each_without_exceeding_budget() {
    let snapshot = fixture(32);
    let mut sent = 0;
    let mut emitted = Vec::new();
    let report = run(
        &snapshot,
        &options(8),
        |request| {
            sent += 1;
            assert_eq!(
                request["state"]["files"][0]["source"],
                crate::syntax::numbered(&snapshot.files[0].source)
            );
            if sent > 1 {
                assert_eq!(request["model"], "jev-pinned");
            }
            Ok(answer(request, &[3, 27]))
        },
        |finding| emitted.push(finding.region.start_line),
    );
    assert_eq!(emitted, vec![3, 27]);
    assert!(report
        .findings
        .iter()
        .all(|finding| finding.status == "predicted" && finding.region.width() == 1));
    assert!(report.search_complete);
    assert!(sent <= 8);
    assert_eq!(report.calls_used, sent);
    assert!(report.calls.iter().any(|call| call.phase == Phase::Verify));
    assert_eq!(report.known_usage.input_tokens, 100 * sent as u64);
}

#[test]
fn exhausted_budget_preserves_unlocalized_candidates_and_zero_calls_never_dispatches() {
    let snapshot = fixture(32);
    for budget in [0, 1, 2] {
        let mut sent = 0;
        let report = run(
            &snapshot,
            &options(budget),
            |request| {
                sent += 1;
                Ok(answer(request, &[3, 27]))
            },
            |_| panic!("nothing verified yet"),
        );
        assert_eq!(sent, budget);
        assert_eq!(report.stop_reason, "call_budget_exhausted");
        assert!(!report.search_complete);
        if budget == 0 {
            assert!(report.unscreened_questions > 0);
        } else {
            for line in [3, 27] {
                assert!(report
                    .findings
                    .iter()
                    .any(|finding| finding.region.start_line <= line
                        && line <= finding.region.end_line));
            }
        }
    }
}

#[test]
fn failed_or_malformed_responses_consume_a_call_and_keep_prior_results() {
    let snapshot = fixture(32);
    for malformed in [false, true] {
        let mut sent = 0;
        let report = run(
            &snapshot,
            &options(8),
            |request| {
                sent += 1;
                if sent == 1 {
                    return Ok(answer(request, &[3]));
                }
                if !malformed {
                    return Err("network failure".into());
                }
                let mut response = answer(request, &[3]);
                response.answers.values_mut().next().unwrap().noul = 1.1;
                Ok(response)
            },
            |_| {},
        );
        assert_eq!(sent, 2);
        assert_eq!(report.calls_used, 2);
        assert_eq!(report.stop_reason, "api_error");
        assert!(report.error.is_some());
        assert_eq!(report.findings.len(), 1);
        assert_eq!(
            report.findings[0].region,
            Span::whole(&snapshot.files[0].source)
        );
    }
}

#[test]
fn byte_limit_stops_before_dispatch_and_large_screening_sets_are_batched() {
    let snapshot = fixture(1);
    let mut small = options(8);
    small.max_bytes = 1;
    let report = run(&snapshot, &small, |_| panic!("must not send"), |_| {});
    assert_eq!(report.stop_reason, "request_limit");
    assert_eq!(report.calls_used, 0);
    assert!(report.unscreened_questions > 0);

    let mut many = fixture(1);
    for index in 0..20 {
        let source = "fn f() {}".to_string();
        many.files.push(SourceFile {
            path: format!("file_{index}.rs"),
            syntax: Syntax::parse(Language::Rust, "f.rs", &source).unwrap(),
            source,
            revision: "working_tree",
        });
    }
    let mut config = options(1);
    config.top_errors = 10;
    let report = run(
        &many,
        &config,
        |request| {
            assert!(request["questions"].as_object().unwrap().len() <= MAX_QUESTIONS);
            Ok(answer(request, &[]))
        },
        |_| {},
    );
    assert_eq!(report.calls_used, 1);
    assert!(report.unscreened_questions > 0);
    assert!(!report.search_complete);
}

#[test]
fn insufficient_context_is_not_reported_as_a_clean_search() {
    let snapshot = fixture(1);
    let report = run(
        &snapshot,
        &options(8),
        |request| {
            let mut response = answer(request, &[]);
            for (key, answer) in &mut response.answers {
                if key.starts_with("context_") {
                    answer.noul = 0.9;
                }
            }
            Ok(response)
        },
        |_| panic!("no supported finding"),
    );
    assert_eq!(report.calls_used, 1);
    assert!(!report.search_complete);
    assert!(report.insufficient_context_files.contains("main.rs"));
}

#[test]
fn final_call_rechecks_localized_findings_before_spending_more_on_broad_regions() {
    let mut snapshot = fixture(32);
    let source = "fn tiny() {}".to_string();
    snapshot.files.push(SourceFile {
        path: "tiny.rs".into(),
        syntax: Syntax::parse(Language::Rust, "tiny.rs", &source).unwrap(),
        source,
        revision: "working_tree",
    });
    let report = run(
        &snapshot,
        &options(2),
        |request| Ok(answer(request, &[1])),
        |_| {},
    );
    assert_eq!(report.calls.last().unwrap().phase, Phase::Verify);
    assert!(report
        .findings
        .iter()
        .any(|finding| finding.file == "tiny.rs" && finding.status == "predicted"));
    assert!(report
        .findings
        .iter()
        .any(|finding| finding.file == "main.rs" && finding.status == "suspected"));
}

#[test]
fn rechecking_can_reject_a_suspicion_without_emitting_it() {
    let snapshot = fixture(1);
    let mut sent = 0;
    let report = run(
        &snapshot,
        &options(8),
        |request| {
            sent += 1;
            Ok(answer(request, if sent == 1 { &[1] } else { &[] }))
        },
        |_| panic!("the recheck rejected the finding"),
    );
    assert_eq!(sent, 2);
    assert!(report.findings.is_empty());
    assert!(report.search_complete);
}
