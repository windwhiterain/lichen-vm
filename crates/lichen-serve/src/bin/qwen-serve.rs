//! An OpenAI-compatible face over the model: a `/v1` surface, one worker, one generation at a time.

use std::sync::Arc;
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

/// How many requests one step answers, which is what the queue may grow to.
const SEQUENCES: usize = 8;

/// How many requests may wait for a place in a batch before the service refuses more.
const QUEUE: usize = 64;

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
}

/// How a job hands its tokens back.
enum Answer {
    /// All of them at the end, which is what a client reading a body waits for.
    Whole(mpsc::Sender<Vec<usize>>),
    /// One at a time as they are chosen, which is what a streaming client reads.
    Stream(tokio::sync::mpsc::UnboundedSender<usize>),
}

/// What the HTTP side shares: the queue in front of the one worker.
#[derive(Clone)]
struct Shared {
    jobs: mpsc::SyncSender<Job>,
    tokenizer: Arc<Tokenizer>,
}

/// The worker: one model, opened once, answering until the queue closes.
///
/// # Invariant
/// A generation is the worker's alone and never returns to the HTTP side, because the model's dispatches
/// are host-blocking calls on one session: a second thread would share the device and the pool with it, and
/// the queue is what turns that into one prompt after another rather than two at once.
fn worker(session: &Session, jobs: mpsc::Receiver<Job>) {
    let Ok(mut model) = Qwen::open_with(session, Width::Narrowed) else {
        return;
    };
    if model.hold_in_memory().is_err() || model.reside(28).is_err() || model.place_head().is_err() {
        return;
    }
    let mut batched = Batched::new(&model);
    let mut waiting: std::collections::HashMap<usize, Answer> = std::collections::HashMap::new();
    let mut closed = false;
    loop {
        // Everything waiting joins the batch, up to the number a step should hold.
        while batched.live() < SEQUENCES && !closed {
            match jobs.try_recv() {
                Ok(job) => {
                    let admitted = batched.admit(&job.prompt, job.limit, job.sampling);
                    match admitted {
                        Ok(id) => {
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
                    if let Ok(id) = batched.admit(&job.prompt, job.limit, job.sampling) {
                        waiting.insert(id, job.answer);
                    }
                }
                Err(_) => break,
            }
            continue;
        }
        let answered = match batched.step() {
            Ok(answered) => answered,
            Err(_) => break,
        };
        for (id, token) in answered {
            if let Some(Answer::Stream(tokens)) = waiting.get(&id) {
                let _ = tokens.send(token);
            }
        }
        let finished: Vec<usize> = waiting
            .keys()
            .copied()
            .filter(|id| batched.done(*id))
            .collect();
        for id in finished {
            let said = batched
                .answer(id)
                .map(<[usize]>::to_vec)
                .unwrap_or_default();
            match waiting.remove(&id) {
                Some(Answer::Whole(reply)) => {
                    let _ = reply.send(said);
                }
                Some(Answer::Stream(_)) | None => {}
            }
            batched.retire(id);
        }
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
    #[serde(default)]
    stream: bool,
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
    #[serde(default)]
    stream: bool,
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
fn generate(
    shared: &Shared,
    prompt: Vec<usize>,
    limit: usize,
    sampling: Sampling,
) -> Result<Vec<usize>, ()> {
    let (reply, answer) = mpsc::channel();
    if shared
        .jobs
        .try_send(Job {
            prompt,
            limit,
            sampling,
            answer: Answer::Whole(reply),
        })
        .is_err()
    {
        return Err(());
    }
    answer.recv().map_err(|_| ())
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
) -> Result<UnboundedReceiverStream<usize>, ()> {
    let (tokens, answer) = tokio::sync::mpsc::unbounded_channel();
    shared
        .jobs
        .try_send(Job {
            prompt,
            limit,
            sampling,
            answer: Answer::Stream(tokens),
        })
        .map_err(|_| ())?;
    Ok(UnboundedReceiverStream::new(answer))
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
    );
    let prompt = chat_prompt(&request.messages);
    let encoded = shared.tokenizer.encode(&prompt);
    let ids = match encoded {
        Ok(ids) if !ids.is_empty() => ids,
        Ok(_) => return refuse(StatusCode::BAD_REQUEST, "the prompt encodes to nothing"),
        Err(refusal) => return refuse(StatusCode::BAD_REQUEST, &refusal.to_string()),
    };
    if request.stream {
        let Ok(tokens) = stream(&shared, ids, limit, sampling) else {
            return refuse(StatusCode::SERVICE_UNAVAILABLE, "the worker is gone");
        };
        let tokenizer = Arc::clone(&shared.tokenizer);
        let mapped = futures_util::StreamExt::map(tokens, move |token| {
            Ok::<_, std::convert::Infallible>(Event::default().data(chat_chunk(&token, &tokenizer)))
        });
        let body = futures_util::StreamExt::chain(
            mapped,
            futures_util::stream::once(async { Ok(Event::default().data("[DONE]")) }),
        );
        return Sse::new(body).into_response();
    }
    let Ok(answered) = generate(&shared, ids.clone(), limit, sampling) else {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "the queue is full");
    };
    let text = shared.tokenizer.decode(&answered);
    Json(json!({
        "id": "chatcmpl-lichen",
        "object": "chat.completion",
        "model": MODEL,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": text },
            "finish_reason": "length",
        }],
        "usage": usage(ids.len(), answered.len()),
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
    );
    let encoded = shared.tokenizer.encode(&request.prompt);
    let ids = match encoded {
        Ok(ids) if !ids.is_empty() => ids,
        Ok(_) => return refuse(StatusCode::BAD_REQUEST, "the prompt encodes to nothing"),
        Err(refusal) => return refuse(StatusCode::BAD_REQUEST, &refusal.to_string()),
    };
    if request.stream {
        let Ok(tokens) = stream(&shared, ids, limit, sampling) else {
            return refuse(StatusCode::SERVICE_UNAVAILABLE, "the worker is gone");
        };
        let tokenizer = Arc::clone(&shared.tokenizer);
        let mapped = futures_util::StreamExt::map(tokens, move |token| {
            let text = tokenizer.decode(std::slice::from_ref(&token));
            Ok::<_, std::convert::Infallible>(
                Event::default().data(
                    json!({
                        "id": "cmpl-lichen",
                        "object": "text_completion",
                        "model": MODEL,
                        "choices": [{ "index": 0, "text": text, "finish_reason": null }],
                    })
                    .to_string(),
                ),
            )
        });
        let body = futures_util::StreamExt::chain(
            mapped,
            futures_util::stream::once(async { Ok(Event::default().data("[DONE]")) }),
        );
        return Sse::new(body).into_response();
    }
    let Ok(answered) = generate(&shared, ids.clone(), limit, sampling) else {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "the queue is full");
    };
    let text = shared.tokenizer.decode(&answered);
    Json(json!({
        "id": "cmpl-lichen",
        "object": "text_completion",
        "model": MODEL,
        "choices": [{ "index": 0, "text": text, "finish_reason": "length" }],
        "usage": usage(ids.len(), answered.len()),
    }))
    .into_response()
}

fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(8080);
    let (jobs, queue) = mpsc::sync_channel(QUEUE);
    let (ready, started) = mpsc::channel();
    std::thread::spawn(move || {
        let session = match Session::open() {
            Ok(session) => session,
            Err(refusal) => {
                eprintln!("no compute backend: {refusal}");
                let _ = ready.send(false);
                return;
            }
        };
        let root = std::env::var("LICHEN_MODEL_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::path::PathBuf::from(".model"));
        eprintln!("the model is loaded; the queue is open");
        let _ = ready.send(true);
        worker(&session, queue);
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
    let shared = Shared { jobs, tokenizer };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("no async runtime: {error}");
            std::process::exit(1);
        }
    };
    runtime.block_on(async move {
        let app = Router::new()
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
