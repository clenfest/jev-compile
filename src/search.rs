use crate::{
    language::{Category, OTHER},
    prediction::{self, Phase, Question, Response, Usage},
    snapshot::{screen_spans, Snapshot},
    syntax::Span,
    Result,
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeSet, VecDeque},
    time::Instant,
};

const ROUTE_THRESHOLD: f64 = 0.5;
const REPORT_THRESHOLD: f64 = 0.85;
const MAX_QUESTIONS: usize = 64;

#[derive(Clone)]
pub struct Options {
    pub model: String,
    pub top_errors: usize,
    pub max_calls: usize,
    pub max_bytes: usize,
}

#[derive(Clone)]
struct Candidate {
    question: Question,
    probability: f64,
}

#[derive(Serialize)]
pub struct Finding {
    pub file: String,
    pub revision: &'static str,
    pub region: Span,
    pub category: &'static str,
    pub message: String,
    pub probability: f64,
    pub status: &'static str,
    pub reason: &'static str,
}

#[derive(Serialize)]
pub struct Call {
    pub number: usize,
    pub phase: Phase,
    pub request_bytes: usize,
    pub omitted_support_declarations: u64,
    pub ambiguous_symbols: usize,
    pub latency_ms: u128,
    pub model: Option<String>,
    pub usage: Option<Usage>,
    pub error: Option<String>,
}

#[derive(Serialize)]
pub struct Report {
    pub status: &'static str,
    pub stop_reason: &'static str,
    pub search_complete: bool,
    pub base_commit: String,
    pub snapshot: String,
    pub language: crate::language::Language,
    pub categories: Vec<Category>,
    pub category_order: &'static str,
    pub route_threshold: f64,
    pub report_threshold: f64,
    pub calls_used: usize,
    pub max_calls: usize,
    pub calls: Vec<Call>,
    pub known_usage: Usage,
    pub screened_questions: usize,
    pub unscreened_questions: usize,
    pub insufficient_context_files: BTreeSet<String>,
    pub warnings: Vec<String>,
    pub findings: Vec<Finding>,
    pub search_ms: u128,
    pub collection_ms: u128,
    pub total_elapsed_ms: u128,
    pub error: Option<String>,
}

enum Job {
    Screen(Question),
    Localize(Candidate),
    Verify(Candidate),
}

impl Job {
    fn tasks(&self, snapshot: &Snapshot) -> Vec<Question> {
        match self {
            Self::Screen(question) => vec![question.clone()],
            Self::Verify(candidate) => vec![candidate.question.clone()],
            Self::Localize(candidate) => snapshot.files[candidate.question.file]
                .syntax
                .partition(candidate.question.region)
                .into_iter()
                .map(|region| Question {
                    region,
                    ..candidate.question.clone()
                })
                .collect(),
        }
    }
}

pub fn categories(snapshot: &Snapshot, options: &Options) -> Vec<Category> {
    let mut categories = snapshot.language.categories(options.top_errors);
    categories.push(OTHER);
    categories
}

fn screen_questions(snapshot: &Snapshot, categories: &[Category]) -> VecDeque<Question> {
    snapshot
        .files
        .iter()
        .enumerate()
        .filter(|(_, source)| source.screen)
        .flat_map(|(file, source)| {
            screen_spans(source).into_iter().flat_map(move |region| {
                (0..categories.len()).map(move |category| Question {
                    file,
                    category,
                    region,
                })
            })
        })
        .collect()
}

fn fitting_batch(
    snapshot: &Snapshot,
    categories: &[Category],
    options: &Options,
    model: &str,
    phase: Phase,
    jobs: impl Iterator<Item = Job>,
) -> Result<(Vec<Job>, Vec<Question>, Value)> {
    let mut batch = Vec::new();
    let mut tasks = Vec::new();
    let mut request = Value::Null;
    for job in jobs {
        let mut proposed = tasks.clone();
        proposed.extend(job.tasks(snapshot));
        let next = prediction::request(snapshot, categories, model, phase, &proposed);
        let size = serde_json::to_vec(&next)?.len();
        if size > options.max_bytes || next["questions"].as_object().unwrap().len() > MAX_QUESTIONS
        {
            if batch.is_empty() {
                return Err(format!("request is {size} bytes, exceeding --max-bytes {} or the {MAX_QUESTIONS}-question batch limit; narrow context or raise --max-bytes", options.max_bytes).into());
            }
            break;
        }
        batch.push(job);
        tasks = proposed;
        request = next;
    }
    Ok((batch, tasks, request))
}

pub fn preview(snapshot: &Snapshot, options: &Options) -> Result<Value> {
    let categories = categories(snapshot, options);
    let (_, _, request) = fitting_batch(
        snapshot,
        &categories,
        options,
        &options.model,
        Phase::Screen,
        screen_questions(snapshot, &categories)
            .into_iter()
            .map(Job::Screen),
    )?;
    Ok(request)
}

fn finding(
    snapshot: &Snapshot,
    categories: &[Category],
    candidate: &Candidate,
    status: &'static str,
    reason: &'static str,
) -> Finding {
    let file = &snapshot.files[candidate.question.file];
    let category = categories[candidate.question.category];
    Finding {
        file: file.path.clone(),
        revision: file.revision,
        region: candidate.question.region,
        category: category.id,
        message: format!(
            "Possible {}: {}",
            category.id.replace('_', " "),
            category.description
        ),
        probability: candidate.probability,
        status,
        reason,
    }
}

pub fn run(
    snapshot: &Snapshot,
    options: &Options,
    mut evaluate: impl FnMut(&Value) -> Result<Response>,
    mut emit: impl FnMut(&Finding),
) -> Report {
    let started = Instant::now();
    let mut effective_options = options.clone();
    let categories = categories(snapshot, options);
    let mut screens = screen_questions(snapshot, &categories);
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut model = options.model.clone();
    let mut report = Report {
        status: "advisory_predictions",
        stop_reason: "completed_scope",
        search_complete: false,
        base_commit: snapshot.base_commit.clone(),
        snapshot: snapshot.fingerprint.clone(),
        language: snapshot.language,
        categories: categories.clone(),
        category_order: "curated; not measured frequency",
        route_threshold: ROUTE_THRESHOLD,
        report_threshold: REPORT_THRESHOLD,
        calls_used: 0,
        max_calls: options.max_calls,
        calls: vec![],
        known_usage: Usage::default(),
        screened_questions: 0,
        unscreened_questions: 0,
        insufficient_context_files: BTreeSet::new(),
        warnings: snapshot.warnings.clone(),
        findings: vec![],
        search_ms: 0,
        collection_ms: 0,
        total_elapsed_ms: 0,
        error: None,
    };
    while !screens.is_empty() || !candidates.is_empty() {
        if report.calls_used == options.max_calls {
            report.stop_reason = "call_budget_exhausted";
            break;
        }
        candidates.sort_by(|left, right| right.probability.total_cmp(&left.probability));
        let leaves = candidates
            .iter()
            .filter(|candidate| candidate.question.region.width() == 1)
            .count();
        // Keep the final available call for checking any already-localized findings.
        let phase = if candidates.is_empty() {
            Phase::Screen
        } else if leaves > 0
            && (options.max_calls - report.calls_used == 1
                || leaves == candidates.len()
                || leaves >= 8)
        {
            Phase::Verify
        } else {
            Phase::Localize
        };
        let jobs: Vec<_> = match phase {
            Phase::Screen => screens.iter().cloned().map(Job::Screen).collect(),
            Phase::Localize => candidates
                .iter()
                .filter(|candidate| candidate.question.region.width() > 1)
                .cloned()
                .map(Job::Localize)
                .collect(),
            Phase::Verify => candidates
                .iter()
                .filter(|candidate| candidate.question.region.width() == 1)
                .cloned()
                .map(Job::Verify)
                .collect(),
        };
        let (batch, tasks, request) = match fitting_batch(
            snapshot,
            &categories,
            &effective_options,
            &model,
            phase,
            jobs.into_iter(),
        ) {
            Ok(batch) => batch,
            Err(error) => {
                report.stop_reason = "request_limit";
                report.error = Some(error.to_string());
                break;
            }
        };
        // Charge at the dispatch boundary. Even transport/validation failures count.
        report.calls_used += 1;
        let call_started = Instant::now();
        let mut receipt = Call {
            number: report.calls_used,
            phase,
            request_bytes: serde_json::to_vec(&request).unwrap().len(),
            omitted_support_declarations: request["state"]["omitted_support"]["declarations"]
                .as_u64()
                .unwrap_or(0),
            ambiguous_symbols: request["state"]["ambiguous_symbols_outside_target_directories"]
                .as_array()
                .map_or(0, Vec::len),
            latency_ms: 0,
            model: None,
            usage: None,
            error: None,
        };
        let response = evaluate(&request).and_then(|response| {
            receipt.model = Some(response.model.clone());
            receipt.usage = Some(response.usage.clone());
            prediction::validate(&request, &response)?;
            Ok(response)
        });
        receipt.latency_ms = call_started.elapsed().as_millis();
        if let Some(usage) = &receipt.usage {
            report.known_usage.input_tokens += usage.input_tokens;
            report.known_usage.output_tokens += usage.output_tokens;
        }
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                receipt.error = Some(error.to_string());
                let token_limit = error
                    .downcast_ref::<prediction::ApiError>()
                    .is_some_and(prediction::ApiError::token_limit);
                let request_bytes = receipt.request_bytes;
                report.calls.push(receipt);
                if token_limit && report.calls_used < options.max_calls {
                    effective_options.max_bytes = request_bytes / 2;
                    report.warnings.push(format!(
                        "Jev rejected a {request_bytes}-byte request with max_tokens_exceeded; reducing the request cap to {} bytes. The rejected attempt consumed one call.",
                        effective_options.max_bytes));
                    continue;
                }
                report.error = Some(error.to_string());
                report.stop_reason = "api_error";
                break;
            }
        };
        model = response.model.clone();
        report.calls.push(receipt);
        let mut offset = 0;
        for job in batch {
            let count = job.tasks(snapshot).len();
            let mut positive = Vec::new();
            for (index, task) in tasks.iter().enumerate().skip(offset).take(count) {
                let probability = response.answers[&format!("q{index}")].noul;
                let context = response.answers[&format!("context_{}", task.file)].noul;
                if context >= ROUTE_THRESHOLD {
                    report
                        .insufficient_context_files
                        .insert(snapshot.files[task.file].path.clone());
                }
                if probability >= ROUTE_THRESHOLD {
                    let candidate = Candidate {
                        question: task.clone(),
                        probability,
                    };
                    if context >= ROUTE_THRESHOLD {
                        report.findings.push(finding(
                            snapshot,
                            &categories,
                            &candidate,
                            "suspected",
                            "insufficient_context",
                        ));
                    } else {
                        positive.push(candidate);
                    }
                }
            }
            offset += count;
            match job {
                Job::Screen(_) => {
                    screens.pop_front();
                    report.screened_questions += 1;
                    candidates.extend(positive);
                }
                Job::Localize(parent) => {
                    remove_candidate(&mut candidates, &parent);
                    if positive.is_empty()
                        && !report
                            .insufficient_context_files
                            .contains(&snapshot.files[parent.question.file].path)
                    {
                        report.findings.push(finding(
                            snapshot,
                            &categories,
                            &parent,
                            "suspected",
                            "localization_inconclusive",
                        ));
                    }
                    candidates.extend(positive);
                }
                Job::Verify(parent) => {
                    remove_candidate(&mut candidates, &parent);
                    for candidate in positive {
                        let verified = candidate.probability >= REPORT_THRESHOLD;
                        let result = finding(
                            snapshot,
                            &categories,
                            &candidate,
                            if verified { "predicted" } else { "suspected" },
                            if verified {
                                "model_rechecked"
                            } else {
                                "verification_inconclusive"
                            },
                        );
                        if verified {
                            emit(&result);
                        }
                        report.findings.push(result);
                    }
                }
            }
        }
    }
    report.unscreened_questions = screens.len();
    report.findings.extend(candidates.iter().map(|candidate| {
        finding(
            snapshot,
            &categories,
            candidate,
            "suspected",
            report.stop_reason,
        )
    }));
    report.findings.sort_by(|left, right| {
        (&left.file, left.region, left.category).cmp(&(&right.file, right.region, right.category))
    });
    report.search_complete = screens.is_empty()
        && report.error.is_none()
        && report.warnings.is_empty()
        && report.insufficient_context_files.is_empty()
        && report
            .calls
            .iter()
            .all(|call| call.omitted_support_declarations == 0 && call.ambiguous_symbols == 0)
        && report
            .findings
            .iter()
            .all(|finding| finding.status == "predicted");
    report.search_ms = started.elapsed().as_millis();
    report
}

fn remove_candidate(candidates: &mut Vec<Candidate>, parent: &Candidate) {
    candidates.retain(|candidate| {
        candidate.question.file != parent.question.file
            || candidate.question.category != parent.question.category
            || candidate.question.region != parent.question.region
    });
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
