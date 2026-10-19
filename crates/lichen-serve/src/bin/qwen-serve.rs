//! An OpenAI-compatible face over the model: a `/v1` surface, one worker, one generation at a time.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Sse;
use axum::response::sse::Event;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};

use tokio_stream::wrappers::UnboundedReceiverStream;

use lichen_serve::{Batched, Qwen, Sampling, Session, Tokenizer, Width};

/// The name this service answers as, which a client uses to pick a model.
const MODEL: &str = "qwen3-1.7b";

/// What the checkpoint spells where a turn ends, which is a stop of its own.
const END_OF_TURN_MARK: &str = "<|im_end|>";

/// How many requests one step answers, which is what the queue may grow to.
const SEQUENCES: usize = 8;

/// How many requests may wait for a place in a batch before the service refuses more.
const QUEUE: usize = 64;

/// The most answers one request may ask for, which the batch holds in its own rows.
const CANDIDATES: usize = 4;

/// One request the worker is asked for.
struct Job {
    /// The prompt's ids, already templated and encoded.
    prompt: Vec<usize>,
    /// How many tokens to answer with.
    limit: usize,
    /// How they are chosen.
    sampling: Sampling,
    /// Where the answer goes.
    answer: Answer,
    /// The strings the request stops at when the answer spells one of them.
    stop: Vec<String>,
    /// How many of each token's rivals the answer should carry, or zero for none.
    rivals: usize,
}

/// What a request may name as a stop, which OpenAI spells as one string or a list of them.
#[derive(Deserialize)]
#[serde(untagged)]
enum StopStrings {
    /// One string.
    One(String),
    /// Several.
    Many(Vec<String>),
}

impl StopStrings {
    /// The strings themselves.
    fn into_vec(self) -> Vec<String> {
        match self {
            StopStrings::One(one) => vec![one],
            StopStrings::Many(many) => many,
        }
    }
}

/// How a job hands its tokens back.
enum Answer {
    /// All of them at the end, which is what a client reading a body waits for.
    Whole(tokio::sync::oneshot::Sender<(Vec<usize>, &'static str, Vec<lichen_serve::Chosen>)>),
    /// One at a time as they are chosen, which is what a streaming client reads.
    Stream(tokio::sync::mpsc::UnboundedSender<Result<usize, &'static str>>),
}

/// What the service has done, which is what `/metrics` reports.
///
/// # Invariant
/// Every counter moves on the side that owns it: the HTTP side counts a refusal it answered, and the worker
/// counts everything the batch did, so a number is never written by two threads at once.
struct Metrics {
    /// Requests taken onto the queue.
    admitted: AtomicU64,
    /// Requests refused because the queue was full.
    refused: AtomicU64,
    /// Requests answered to their end.
    finished: AtomicU64,
    /// Tokens answered.
    tokens: AtomicU64,
    /// Batch steps taken.
    steps: AtomicU64,
    /// Requests live in the batch now.
    live: AtomicU64,
    /// Time spent inside batch steps, in nanoseconds.
    step_nanos: AtomicU64,
    /// Requests whose wall time has been counted.
    timed: AtomicU64,
    /// The sum of their wall times, in nanoseconds.
    request_nanos: AtomicU64,
    /// How many requests finished within each power of two of microseconds.
    buckets: [AtomicU64; LADDER],
}

/// How many quarter-powers of two a request's duration is counted across.
///
/// # Invariant
/// The bounds are quarters of a power of two, from a microsecond to hours: a bucket a whole power wide
/// whole power wide is too coarse to tell one request's time from another's once a batch is answering.
const LADDER: usize = 4 * 32;

/// The upper bound of bucket `rung`, in microseconds.
fn rung(rung: usize) -> u64 {
    let whole = (rung / 4) as u32;
    let quarter = (rung % 4) as u32;
    // A quarter of a power of two is its fourth root, which is one of four constants.
    let scale = [
        1.0_f64,
        1.189_207_115_002_721_1,
        1.414_213_562_373_095_1,
        1.681_792_830_507_429_6,
    ];
    (2f64.powi(whole as i32) * scale[quarter as usize]) as u64
}

impl Default for Metrics {
    /// Every counter at rest, with a bucket for every rung of the ladder.
    fn default() -> Self {
        Self {
            admitted: AtomicU64::new(0),
            refused: AtomicU64::new(0),
            finished: AtomicU64::new(0),
            tokens: AtomicU64::new(0),
            steps: AtomicU64::new(0),
            live: AtomicU64::new(0),
            step_nanos: AtomicU64::new(0),
            timed: AtomicU64::new(0),
            request_nanos: AtomicU64::new(0),
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

impl Metrics {
    /// One line of the exposition format.
    fn line(&self, name: &str, kind: &str, help: &str, value: u64) -> String {
        format!(
            "# HELP {name} {help}
# TYPE {name} {kind}
{name} {value}
"
        )
    }

    /// Every counter, in the exposition format a scraper reads.
    /// Count one request's wall time, in nanoseconds.
    fn timed(&self, nanos: u64) {
        self.timed.fetch_add(1, Ordering::Relaxed);
        self.request_nanos.fetch_add(nanos, Ordering::Relaxed);
        let micros = (nanos / 1000).max(1);
        let at = (1..LADDER)
            .find(|rung_index| micros <= rung(*rung_index))
            .unwrap_or(LADDER - 1);
        self.buckets[at].fetch_add(1, Ordering::Relaxed);
    }

    /// The request durations, as the histogram a scraper reads.
    fn render_durations(&self) -> String {
        let mut text = String::from(
            "# HELP lichen_request_duration_seconds a request's time from the queue to its answer
",
        );
        text += "# TYPE lichen_request_duration_seconds histogram
";
        // A bucket's count is everything that came in under it, not only that interval of it.
        let mut held = 0;
        for (at, bucket) in self.buckets.iter().enumerate() {
            held += bucket.load(Ordering::Relaxed);
            let bound = rung(at) as f64 / 1_000_000.0;
            text += &format!(
                "lichen_request_duration_seconds_bucket{{le=\"{bound}\"}} {held}
"
            );
        }
        let counted = self.timed.load(Ordering::Relaxed);
        let total = self.request_nanos.load(Ordering::Relaxed);
        text += &format!(
            "lichen_request_duration_seconds_bucket{{le=\"+Inf\"}} {counted}
"
        );
        text += &format!(
            "lichen_request_duration_seconds_count {counted}
"
        );
        text += &format!(
            "lichen_request_duration_seconds_sum {:.6}
",
            total as f64 / 1_000_000_000.0
        );
        text
    }

    fn render(&self) -> String {
        let mut text = String::new();
        text += &self.line(
            "lichen_requests_admitted",
            "counter",
            "requests taken onto the queue",
            self.admitted.load(Ordering::Relaxed),
        );
        text += &self.line(
            "lichen_requests_refused",
            "counter",
            "requests refused because the queue was full",
            self.refused.load(Ordering::Relaxed),
        );
        text += &self.line(
            "lichen_requests_finished",
            "counter",
            "requests answered to their end",
            self.finished.load(Ordering::Relaxed),
        );
        text += &self.line(
            "lichen_tokens_answered",
            "counter",
            "tokens answered",
            self.tokens.load(Ordering::Relaxed),
        );
        text += &self.line(
            "lichen_batch_steps",
            "counter",
            "batch steps taken",
            self.steps.load(Ordering::Relaxed),
        );
        text += &self.line(
            "lichen_batch_live",
            "gauge",
            "requests live in the batch",
            self.live.load(Ordering::Relaxed),
        );
        text += &self.render_durations();
        text += &self.line(
            "lichen_step_nanoseconds",
            "counter",
            "time spent inside batch steps",
            self.step_nanos.load(Ordering::Relaxed),
        );
        text
    }
}

/// What the HTTP side shares: the queue in front of the one worker.
#[derive(Clone)]
struct Shared {
    jobs: mpsc::SyncSender<Job>,
    tokenizer: Arc<Tokenizer>,
    metrics: Arc<Metrics>,
}

/// The worker: one model, opened once, answering until the queue closes.
///
/// # Invariant
/// A generation is the worker's alone and never returns to the HTTP side, because the model's dispatches
/// are host-blocking calls on one session: a second thread would share the device and the pool with it, and
/// the queue is what turns that into one prompt after another rather than two at once.
fn worker(
    session: &Session,
    jobs: mpsc::Receiver<Job>,
    tokenizer: Arc<Tokenizer>,
    metrics: Arc<Metrics>,
) {
    let Ok(mut model) = Qwen::open_with(session, Width::Narrowed) else {
        return;
    };
    if model.hold_in_memory().is_err() || model.reside(28).is_err() || model.place_head().is_err() {
        return;
    }
    let mut batched = Batched::new(&model);
    let mut waiting: std::collections::HashMap<usize, Answer> = std::collections::HashMap::new();
    // The requests that met a stop of their own, and are retired at the end of the step.
    let mut stopped: std::collections::HashMap<usize, &'static str> =
        std::collections::HashMap::new();
    let mut stops: std::collections::HashMap<usize, Vec<String>> = std::collections::HashMap::new();
    // What each request's tokens were, in order, so an answer can carry how likely they were.
    let mut picks: std::collections::HashMap<usize, Vec<lichen_serve::Chosen>> =
        std::collections::HashMap::new();
    let mut closed = false;
    loop {
        // Everything waiting joins the batch, up to the number a step should hold.
        while batched.live() < SEQUENCES && !closed {
            match jobs.try_recv() {
                Ok(job) => {
                    let admitted = batched.admit(&job.prompt, job.limit, job.sampling, job.rivals);
                    match admitted {
                        Ok(id) => {
                            metrics.admitted.fetch_add(1, Ordering::Relaxed);
                            stops.insert(id, job.stop);
                            waiting.insert(id, job.answer);
                        }
                        Err(_) => drop(job.answer),
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    closed = true;
                    break;
                }
            }
        }
        if batched.live() == 0 {
            if closed {
                break;
            }
            match jobs.recv() {
                Ok(job) => {
                    if let Ok(id) = batched.admit(&job.prompt, job.limit, job.sampling, job.rivals)
                    {
                        metrics.admitted.fetch_add(1, Ordering::Relaxed);
                        stops.insert(id, job.stop);
                        waiting.insert(id, job.answer);
                    }
                }
                Err(_) => break,
            }
            continue;
        }
        let started = std::time::Instant::now();
        let answered = match batched.step() {
            Ok(answered) => answered,
            Err(_) => break,
        };
        metrics.steps.fetch_add(1, Ordering::Relaxed);
        metrics
            .step_nanos
            .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        metrics
            .tokens
            .fetch_add(answered.len() as u64, Ordering::Relaxed);
        metrics.live.store(batched.live() as u64, Ordering::Relaxed);
        for (id, chosen) in answered {
            let token = chosen.token;
            picks.entry(id).or_default().push(chosen);
            if let Some(Answer::Stream(tokens)) = waiting.get(&id) {
                let _ = tokens.send(Ok(token));
            }
            // A turn ends where its marker is spelled, matched as text rather than as one id.
            let said = batched
                .answer(id)
                .map(<[usize]>::to_vec)
                .unwrap_or_default();
            let spelled = tokenizer.decode(&said);
            let named = stops
                .get(&id)
                .is_some_and(|wanted| wanted.iter().any(|stop| spelled.contains(stop.as_str())));
            if named {
                stopped.insert(id, "stop");
            }
            if spelled.contains(END_OF_TURN_MARK) {
                stopped.insert(id, "stop");
            }
        }
        let finished: Vec<usize> = waiting
            .keys()
            .copied()
            .filter(|id| batched.done(*id) || stopped.contains_key(id))
            .collect();
        for id in finished {
            let said = batched
                .answer(id)
                .map(<[usize]>::to_vec)
                .unwrap_or_default();
            // The turn's end token is a stop of its own, which no client has to name.
            let reason = stopped.remove(&id).unwrap_or("length");
            match waiting.remove(&id) {
                Some(Answer::Whole(reply)) => {
                    let _ = reply.send((said, reason, picks.remove(&id).unwrap_or_default()));
                }
                Some(Answer::Stream(tokens)) => {
                    let _ = tokens.send(Err(reason));
                }
                None => {}
            }
            stops.remove(&id);
            picks.remove(&id);
            batched.retire(id);
            metrics.finished.fetch_add(1, Ordering::Relaxed);
        }
        metrics.live.store(batched.live() as u64, Ordering::Relaxed);
    }
}

/// A message as an OpenAI request spells it.
#[derive(Deserialize)]
struct Message {
    role: String,
    content: String,
}

/// A chat request's body, of the fields this service reads.
#[derive(Deserialize)]
struct ChatRequest {
    messages: Vec<Message>,
    #[serde(default)]
    max_tokens: Option<usize>,
    #[serde(default)]
    temperature: Option<f32>,
    #[serde(default)]
    top_k: Option<usize>,
    /// The nucleus a request asks for.
    #[serde(default)]
    top_p: Option<f32>,
    /// How many answers a request wants, each chosen on its own.
    #[serde(default)]
    n: Option<usize>,
    /// Whether the answer should carry how likely each of its tokens was.
    #[serde(default)]
    logprobs: Option<bool>,
    /// How many of each token's rivals that answer should show.
    #[serde(default)]
    top_logprobs: Option<usize>,
    #[serde(default)]
    stream: bool,
    /// Where the answer stops, which a client may name.
    #[serde(default)]
    stop: Option<StopStrings>,
}

/// A completion request's body.
#[derive(Deserialize)]
struct CompletionRequest {
    prompt: String,
    #[serde(default)]
    max_tokens: Option<usize>,
    #[serde(default)]
    temperature: Option<f32>,
    #[serde(default)]
    top_k: Option<usize>,
    /// The nucleus a request asks for.
    #[serde(default)]
    top_p: Option<f32>,
    /// How many answers a request wants, each chosen on its own.
    #[serde(default)]
    n: Option<usize>,
    /// Whether the answer should carry how likely each of its tokens was.
    #[serde(default)]
    logprobs: Option<bool>,
    /// How many of each token's rivals that answer should show.
    #[serde(default)]
    top_logprobs: Option<usize>,
    #[serde(default)]
    stream: bool,
    /// Where the answer stops, which a client may name.
    #[serde(default)]
    stop: Option<StopStrings>,
}

/// The prompt a chat request becomes, in the template the checkpoint was tuned on.
///
/// # Invariant
/// The template is the checkpoint's own: `<|im_start|>role` then the content then `<|im_end|>`, ending with
/// the assistant's opening, because the special tokens are what the model was trained to see around a turn.
fn chat_prompt(messages: &[Message]) -> String {
    let mut prompt = String::new();
    for message in messages {
        prompt.push_str("<|im_start|>");
        prompt.push_str(&message.role);
        prompt.push('\n');
        prompt.push_str(&message.content);
        prompt.push_str("<|im_end|>\n");
    }
    prompt.push_str("<|im_start|>assistant\n");
    prompt
}

/// The usage an OpenAI client reads, from the counts this service knows.
fn usage(prompt: usize, completion: usize) -> Value {
    json!({
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "total_tokens": prompt + completion,
    })
}

/// Refuse in the shape an OpenAI client reads: an error object and a status.
fn refuse(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": "invalid_request_error",
                "code": status.as_u16(),
            }
        })),
    )
        .into_response()
}

/// Ask the worker for `limit` tokens and wait for them.
async fn generate(
    shared: &Shared,
    prompt: Vec<usize>,
    limit: usize,
    sampling: Sampling,
    stop: Vec<String>,
    wanted: usize,
    rivals: usize,
) -> Result<Vec<(Vec<usize>, &'static str, Vec<lichen_serve::Chosen>)>, ()> {
    let started = std::time::Instant::now();
    // Several answers are the same prompt sent as several rows, answered in the same steps.
    let mut replies = Vec::with_capacity(wanted.max(1));
    for candidate in 0..wanted.max(1) {
        let (reply, answer) = tokio::sync::oneshot::channel();
        // Each candidate draws from its own stream, or every answer of one request would be the same.
        let seed = sampling.seed.wrapping_add(candidate as u64);
        let sampling = Sampling { seed, ..sampling };
        if shared
            .jobs
            .try_send(Job {
                prompt: prompt.clone(),
                limit,
                sampling,
                answer: Answer::Whole(reply),
                stop: stop.clone(),
                rivals,
            })
            .is_err()
        {
            shared.metrics.refused.fetch_add(1, Ordering::Relaxed);
            return Err(());
        }
        replies.push(answer);
    }
    let mut answers = Vec::with_capacity(replies.len());
    for answer in replies {
        answers.push(answer.await.map_err(|_| ())?);
    }
    shared.metrics.timed(started.elapsed().as_nanos() as u64);
    Ok(answers)
}

/// Ask the worker for `limit` tokens and hand them on as they arrive.
///
/// # Invariant
/// The stream ends when the worker drops its sender, which is when the generation ends: a client that reads
/// to the end has read every token the worker chose, and the `[DONE]` the protocol wants is appended after
/// the last of them rather than sent through the queue.
fn stream(
    shared: &Shared,
    prompt: Vec<usize>,
    limit: usize,
    sampling: Sampling,
    stop: Vec<String>,
) -> Result<UnboundedReceiverStream<Result<usize, &'static str>>, ()> {
    let (tokens, answer) = tokio::sync::mpsc::unbounded_channel::<Result<usize, &'static str>>();
    shared
        .jobs
        .try_send(Job {
            prompt,
            limit,
            sampling,
            answer: Answer::Stream(tokens),
            stop,
            rivals: 0,
        })
        .map_err(|_| ())?;
    Ok(UnboundedReceiverStream::new(answer))
}

/// The chunk that says a streamed answer ended, which is what a client reads as the stop.
fn finished_chunk(reason: &'static str) -> String {
    json!({
        "id": "chatcmpl-lichen",
        "object": "chat.completion.chunk",
        "model": MODEL,
        "choices": [{ "index": 0, "delta": {}, "finish_reason": reason }],
    })
    .to_string()
}

/// The tokens of an answer with how likely each was, or nothing when no rivals were asked for.
///
/// # Invariant
/// A token is one entry even when its text is several characters: the entry names the token's own spelling,
/// which is what a client reading log probabilities measures against.
fn logprob_content(
    tokenizer: &Tokenizer,
    picks: &[lichen_serve::Chosen],
    rivals: usize,
) -> Option<Vec<Value>> {
    // A greedy answer keeps one candidate whether or not a client asked, so the request decides.
    if rivals == 0 {
        return None;
    }
    let first = picks.first()?;
    if first.top.is_empty() {
        return None;
    }
    Some(
        picks
            .iter()
            .map(|chosen| {
                let rivals: Vec<Value> = chosen
                    .top
                    .iter()
                    .map(|(index, logprob)| {
                        json!({
                            "token": tokenizer.decode(std::slice::from_ref(index)),
                            "logprob": logprob,
                            "bytes": Value::Null,
                        })
                    })
                    .collect();
                json!({
                    "token": tokenizer.decode(std::slice::from_ref(&chosen.token)),
                    "logprob": chosen.logprob,
                    "bytes": Value::Null,
                    "top_logprobs": rivals,
                })
            })
            .collect(),
    )
}

/// How many rivals an answer shows, or zero when a request did not ask.
///
/// # Invariant
/// A request that forgot `top_logprobs` still sees the tokens it got; the rivals are what a count adds.
fn wanted_rivals(asked: &Option<bool>, rivals: &Option<usize>) -> usize {
    if asked.unwrap_or(false) {
        rivals.unwrap_or(0).min(20) + 1
    } else {
        0
    }
}

/// The text `ids` spell, cut where the turn's own marker begins.
///
/// # Invariant
/// The marker is not part of the answer OpenAI's surface returns, so the cut happens here: a client that
/// saw it would read the checkpoint's control token as content.
fn answered_text(tokenizer: &Tokenizer, ids: &[usize], stop: &[String]) -> String {
    let text = tokenizer.decode(ids);
    let mut end = text.len();
    if let Some(at) = text.find(END_OF_TURN_MARK) {
        end = end.min(at);
    }
    for wanted in stop {
        if let Some(at) = text.find(wanted.as_str()) {
            end = end.min(at);
        }
    }
    text[..end].to_string()
}

/// One token as a chat chunk, which is the shape a streaming client appends.
fn chat_chunk(token: &usize, tokenizer: &Tokenizer) -> String {
    let text = tokenizer.decode(std::slice::from_ref(token));
    json!({
        "id": "chatcmpl-lichen",
        "object": "chat.completion.chunk",
        "model": MODEL,
        "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": null }],
    })
    .to_string()
}

async fn metrics(State(shared): State<Shared>) -> String {
    shared.metrics.render()
}

async fn models() -> Json<Value> {
    Json(json!({
        "object": "list",
        "data": [{
            "id": MODEL,
            "object": "model",
            "owned_by": "lichen",
        }],
    }))
}

async fn chat(State(shared): State<Shared>, Json(request): Json<ChatRequest>) -> Response {
    let limit = request.max_tokens.unwrap_or(64).min(512);
    let sampling = Sampling::sampled(
        request.temperature.unwrap_or(0.0),
        request.top_k.unwrap_or(0),
        7,
    )
    .with_top_p(request.top_p.unwrap_or(1.0));
    let prompt = chat_prompt(&request.messages);
    let encoded = shared.tokenizer.encode(&prompt);
    let ids = match encoded {
        Ok(ids) if !ids.is_empty() => ids,
        Ok(_) => return refuse(StatusCode::BAD_REQUEST, "the prompt encodes to nothing"),
        Err(refusal) => return refuse(StatusCode::BAD_REQUEST, &refusal.to_string()),
    };
    if request.stream {
        if request.n.unwrap_or(1) > 1 {
            return refuse(
                StatusCode::BAD_REQUEST,
                "`n` above one is not offered with `stream`",
            );
        }
        if request.logprobs.unwrap_or(false) {
            return refuse(
                StatusCode::BAD_REQUEST,
                "`logprobs` is not offered with `stream`",
            );
        }
        let stops = request.stop.map(StopStrings::into_vec).unwrap_or_default();
        let Ok(tokens) = stream(&shared, ids, limit, sampling, stops) else {
            return refuse(StatusCode::SERVICE_UNAVAILABLE, "the worker is gone");
        };
        let tokenizer = Arc::clone(&shared.tokenizer);
        let mapped = futures_util::StreamExt::map(tokens, move |token| {
            let chunk = match token {
                Ok(token) => chat_chunk(&token, &tokenizer),
                Err(reason) => finished_chunk(reason),
            };
            Ok::<_, std::convert::Infallible>(Event::default().data(chunk))
        });
        let body = futures_util::StreamExt::chain(
            mapped,
            futures_util::stream::once(async { Ok(Event::default().data("[DONE]")) }),
        );
        return Sse::new(body).into_response();
    }
    let stops = request.stop.map(StopStrings::into_vec).unwrap_or_default();
    let wanted = request.n.unwrap_or(1).clamp(1, CANDIDATES);
    let rivals = wanted_rivals(&request.logprobs, &request.top_logprobs);
    let Ok(answers) = generate(
        &shared,
        ids.clone(),
        limit,
        sampling,
        stops.clone(),
        wanted,
        rivals,
    )
    .await
    else {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "the queue is full");
    };
    let choices: Vec<Value> = answers
        .iter()
        .enumerate()
        .map(|(index, (answered, reason, picks))| {
            let mut choice = json!({
                "index": index,
                "message": {
                    "role": "assistant",
                    "content": answered_text(&shared.tokenizer, answered, &stops),
                },
                "finish_reason": reason,
            });
            if let Some(content) = logprob_content(&shared.tokenizer, picks, rivals) {
                choice["logprobs"] = json!({ "content": content });
            }
            choice
        })
        .collect();
    let answered: usize = answers.iter().map(|(said, _, _)| said.len()).sum();
    Json(json!({
        "id": "chatcmpl-lichen",
        "object": "chat.completion",
        "model": MODEL,
        "choices": choices,
        "usage": usage(ids.len(), answered),
    }))
    .into_response()
}

async fn complete(
    State(shared): State<Shared>,
    Json(request): Json<CompletionRequest>,
) -> Response {
    let limit = request.max_tokens.unwrap_or(64).min(512);
    let sampling = Sampling::sampled(
        request.temperature.unwrap_or(0.0),
        request.top_k.unwrap_or(0),
        7,
    )
    .with_top_p(request.top_p.unwrap_or(1.0));
    let encoded = shared.tokenizer.encode(&request.prompt);
    let ids = match encoded {
        Ok(ids) if !ids.is_empty() => ids,
        Ok(_) => return refuse(StatusCode::BAD_REQUEST, "the prompt encodes to nothing"),
        Err(refusal) => return refuse(StatusCode::BAD_REQUEST, &refusal.to_string()),
    };
    if request.stream {
        if request.n.unwrap_or(1) > 1 {
            return refuse(
                StatusCode::BAD_REQUEST,
                "`n` above one is not offered with `stream`",
            );
        }
        if request.logprobs.unwrap_or(false) {
            return refuse(
                StatusCode::BAD_REQUEST,
                "`logprobs` is not offered with `stream`",
            );
        }
        let stops = request.stop.map(StopStrings::into_vec).unwrap_or_default();
        let Ok(tokens) = stream(&shared, ids, limit, sampling, stops) else {
            return refuse(StatusCode::SERVICE_UNAVAILABLE, "the worker is gone");
        };
        let tokenizer = Arc::clone(&shared.tokenizer);
        let mapped = futures_util::StreamExt::map(tokens, move |token| {
            let payload = match token {
                Ok(token) => json!({
                    "id": "cmpl-lichen",
                    "object": "text_completion",
                    "model": MODEL,
                    "choices": [{
                        "index": 0,
                        "text": tokenizer.decode(std::slice::from_ref(&token)),
                        "finish_reason": null,
                    }],
                }),
                Err(reason) => json!({
                    "id": "cmpl-lichen",
                    "object": "text_completion",
                    "model": MODEL,
                    "choices": [{ "index": 0, "text": "", "finish_reason": reason }],
                }),
            };
            Ok::<_, std::convert::Infallible>(Event::default().data(payload.to_string()))
        });
        let body = futures_util::StreamExt::chain(
            mapped,
            futures_util::stream::once(async { Ok(Event::default().data("[DONE]")) }),
        );
        return Sse::new(body).into_response();
    }
    let stops = request.stop.map(StopStrings::into_vec).unwrap_or_default();
    let wanted = request.n.unwrap_or(1).clamp(1, CANDIDATES);
    let rivals = wanted_rivals(&request.logprobs, &request.top_logprobs);
    let Ok(answers) = generate(
        &shared,
        ids.clone(),
        limit,
        sampling,
        stops.clone(),
        wanted,
        rivals,
    )
    .await
    else {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "the queue is full");
    };
    let choices: Vec<Value> = answers
        .iter()
        .enumerate()
        .map(|(index, (answered, reason, picks))| {
            let mut choice = json!({
                "index": index,
                "text": answered_text(&shared.tokenizer, answered, &stops),
                "finish_reason": reason,
            });
            if let Some(content) = logprob_content(&shared.tokenizer, picks, rivals) {
                choice["logprobs"] = json!({ "content": content });
            }
            choice
        })
        .collect();
    let counted: usize = answers.iter().map(|(said, _, _)| said.len()).sum();
    Json(json!({
        "id": "cmpl-lichen",
        "object": "text_completion",
        "model": MODEL,
        "choices": choices,
        "usage": usage(ids.len(), counted),
    }))
    .into_response()
}

fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(8080);
    let root = std::env::var("LICHEN_MODEL_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(".model"));
    let tokenizer = match Tokenizer::open(&root.join("tokenizer.json")) {
        Ok(tokenizer) => Arc::new(tokenizer),
        Err(refusal) => {
            eprintln!("{refusal}");
            std::process::exit(1);
        }
    };
    let (jobs, queue) = mpsc::sync_channel(QUEUE);
    let (ready, started) = mpsc::channel();
    // The name the handler answers to is the route's, so the counters are the one thing named apart.
    let counters = Arc::new(Metrics::default());
    let counted = Arc::clone(&counters);
    let spoken = Arc::clone(&tokenizer);
    std::thread::spawn(move || {
        let session = match Session::open() {
            Ok(session) => session,
            Err(refusal) => {
                eprintln!("no compute backend: {refusal}");
                let _ = ready.send(false);
                return;
            }
        };
        eprintln!("the model is loaded; the queue is open");
        let _ = ready.send(true);
        worker(&session, queue, spoken, counted);
    });
    if !started.recv().unwrap_or(false) {
        std::process::exit(1);
    }
    let root = std::env::var("LICHEN_MODEL_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(".model"));
    let tokenizer = match Tokenizer::open(&root.join("tokenizer.json")) {
        Ok(tokenizer) => Arc::new(tokenizer),
        Err(refusal) => {
            eprintln!("{refusal}");
            std::process::exit(1);
        }
    };
    let shared = Shared {
        jobs,
        tokenizer,
        metrics: Arc::clone(&counters),
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("no async runtime: {error}");
            std::process::exit(1);
        }
    };
    runtime.block_on(async move {
        let app = Router::new()
            .route("/metrics", get(metrics))
            .route("/v1/models", get(models))
            .route("/v1/chat/completions", post(chat))
            .route("/v1/completions", post(complete))
            .with_state(shared);
        let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("cannot listen on {port}: {error}");
                return;
            }
        };
        eprintln!("listening on http://127.0.0.1:{port}/v1");
        let _ = axum::serve(listener, app).await;
    });
}
