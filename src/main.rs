/////////////////////////////////////////////////////////////
// src/main.rs
//
// A single Actix Web server that records 5s of audio
// in memory. It can switch between macOS "rec" (SoX)
// and Linux "arecord" based on MIC_BACKEND env var.
//
// Then sends the captured WAV data to OpenAI Whisper & GPT.
// Logging has been expanded so you can confirm local calls.
//
// ADDED:
// - Chunk-based continuous recording in 5s blocks until user 
//   clicks "Stop". 
// - Appending transcripts and GPT responses as JSON to a file,
//   with "source": "Microphone" or "OPENAI RESPONSE" plus 
//   timestamps.
//
// NEW SSE CHANGES:
// - A broadcast channel in AppState (log_sender) for streaming 
//   appended lines in real time.
// - A new /live_log SSE endpoint that sends each appended JSON line
//   to the browser without requiring refresh.
//
// ADDITION:
// - We now keep up to the last 20 messages in conversation_history 
//   to provide context to GPT each time we process a new chunk.
/////////////////////////////////////////////////////////////

use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use std::env;
use std::sync::Arc;
use std::fs;

use tokio::sync::{Mutex as AsyncMutex, broadcast};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use anyhow::{Context, Result};
use serde::Serialize;
use std::process::Stdio;

// ADDED: for timestamps
use chrono::Utc;

// For streaming lines as SSE
use futures_util::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use actix_web::web::{Data, Bytes};

/////////////////////////////////////////////////////////////
// For HTTP calls to OpenAI
/////////////////////////////////////////////////////////////
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};

/////////////////////////////////////////////////////////////
// Shared state (in an Actix Web Data wrapper).
/////////////////////////////////////////////////////////////
struct AppState {
    // If we're currently recording
    is_recording: Arc<AsyncMutex<bool>>,
    // Last transcription from Whisper
    last_transcript: Arc<AsyncMutex<String>>,
    // Last GPT response to that transcription
    last_gpt_response: Arc<AsyncMutex<String>>,

    // SSE broadcast
    log_sender: broadcast::Sender<String>,

    // NEW: store up to last 20 conversation messages
    // Each tuple is (role, content), role is "user" or "assistant"
    conversation_history: Arc<AsyncMutex<Vec<(String, String)>>>,
}

/////////////////////////////////////////////////////////////
// GET /  => Serve static/index.html
/////////////////////////////////////////////////////////////
#[get("/")]
async fn index() -> impl Responder {
    println!("▶ GET / - Serving static/index.html...");

    match fs::read_to_string("static/index.html") {
        Ok(html) => HttpResponse::Ok().content_type("text/html").body(html),
        Err(_) => HttpResponse::NotFound().body("<h1>index.html not found</h1>"),
    }
}

/////////////////////////////////////////////////////////////
// GET /transcript
//
// Returns JSON with the last transcript and GPT response
/////////////////////////////////////////////////////////////
#[derive(Serialize)]
struct TranscriptResponse {
    transcript: String,
    gpt_response: String,
}

#[get("/transcript")]
async fn get_transcript(app_data: web::Data<AppState>) -> impl Responder {
    let transcript = app_data.last_transcript.lock().await.clone();
    let gpt_resp = app_data.last_gpt_response.lock().await.clone();

    HttpResponse::Ok().json(TranscriptResponse {
        transcript,
        gpt_response: gpt_resp,
    })
}

/////////////////////////////////////////////////////////////
// POST /start_recording
//
// If not already recording, spawns a background task to:
//   1) Repeatedly capture 5s of audio -> in-memory
//   2) Transcribe via Whisper
//   3) Summarize via GPT
//   4) Append each chunk+response to a local JSON file
//   5) Update the shared transcript/gpt fields
// until user calls /stop_recording
/////////////////////////////////////////////////////////////
#[post("/start_recording")]
async fn start_recording(app_data: web::Data<AppState>) -> impl Responder {
    println!("▶ POST /start_recording - Checking if we're already recording...");

    let mut recording_flag = app_data.is_recording.lock().await;
    if *recording_flag {
        println!("   Already recording!");
        return HttpResponse::Ok().body("Already recording");
    }

    // Mark ourselves as recording
    *recording_flag = true;
    println!("   Setting is_recording = true, spawning background task...");

    let shared_state = app_data.clone();
    tokio::spawn(async move {
        if let Err(e) = record_and_process_audio(shared_state).await {
            println!("   ERROR: record_and_process_audio => {:?}", e);
        }
    });

    HttpResponse::Ok().body("Recording started in memory for 5s blocks...")
}

/////////////////////////////////////////////////////////////
// POST /stop_recording
//
// Sets is_recording = false. We do NOT forcibly kill the
// mic process if it's mid-block (the chunk will wrap up
// once the 5s finishes).
/////////////////////////////////////////////////////////////
#[post("/stop_recording")]
async fn stop_recording(app_data: web::Data<AppState>) -> impl Responder {
    println!("▶ POST /stop_recording - Setting is_recording = false...");
    let mut recording_flag = app_data.is_recording.lock().await;
    *recording_flag = false;

    HttpResponse::Ok().body("Recording stopped")
}

/////////////////////////////////////////////////////////////
// MAIN - start Actix web server on port from $PORT or 8080
/////////////////////////////////////////////////////////////
#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Choose port from $PORT or default 8080
    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|val| val.parse().ok())
        .unwrap_or(8080);

    println!("===============================================");
    println!("🚀 Starting in-memory Audio -> Whisper -> GPT!");
    println!("   Listening on port {}", port);
    println!("===============================================");

    // ADDED: Create a broadcast channel for real-time SSE lines
    let (log_sender, _rx) = broadcast::channel(100);

    // NEW: Initialize conversation_history
    let conversation_history = Arc::new(AsyncMutex::new(Vec::new()));

    // Initialize shared state
    let app_state = web::Data::new(AppState {
        is_recording: Arc::new(AsyncMutex::new(false)),
        last_transcript: Arc::new(AsyncMutex::new(String::new())),
        last_gpt_response: Arc::new(AsyncMutex::new(String::new())),
        log_sender,
        conversation_history,
    });

    // Launch Actix Web
    HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .service(index)
            .service(get_transcript)
            .service(start_recording)
            .service(stop_recording)
            .service(conversation_log) // ADDED
            .service(live_log_sse)     // ADDED SSE route
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}

/////////////////////////////////////////////////////////////
// record_and_process_audio
//
// ADDED: Now runs in a loop, capturing 5s chunks while 
// is_recording = true. For each chunk, we do:
// 1) record_audio_in_memory(5)
// 2) transcribe with Whisper
// 3) build a chat prompt with last 20 messages + new transcript
// 4) Summarize with GPT
// 5) append both to a JSON file with timestamps
// 6) update shared state
/////////////////////////////////////////////////////////////
async fn record_and_process_audio(app_data: web::Data<AppState>) -> Result<()> {
    // We loop until is_recording = false
    loop {
        {
            let flag = app_data.is_recording.lock().await;
            if !*flag {
                println!("   >>> Recording loop ended (user clicked Stop).");
                break;
            }
        }

        println!("   >>> Starting 5s in-memory recording chunk...");
        let audio_data = record_audio_in_memory(5).await?;
        println!("   >>> Chunk captured, {} bytes.", audio_data.len());

        // Transcribe
        println!("   >>> Sending chunk to Whisper...");
        let transcript = transcribe_audio_with_whisper(&audio_data).await?;
        println!("   >>> Transcript: {}", transcript);

        // We add this new user message to conversation history
        {
            let mut hist = app_data.conversation_history.lock().await;
            hist.push(("user".to_string(), transcript.clone()));
            // Keep only last 20 messages
            if hist.len() > 40 {
                // each user+assistant = 2 messages, so 40 entries ~ 20 pairs
                hist.drain(0..(hist.len() - 40));
            }
        }

        // Summarize with GPT using last 20 messages
        println!("   >>> Summarizing chunk with GPT...");
        let gpt_response = summarize_with_gpt(&app_data, &transcript).await?;
        println!("   >>> GPT response: {}", gpt_response);

        // Add the assistant's response to conversation history
        {
            let mut hist = app_data.conversation_history.lock().await;
            hist.push(("assistant".to_string(), gpt_response.clone()));
            if hist.len() > 40 {
                hist.drain(0..(hist.len() - 40));
            }
        }

        // Append to JSON file for logging
        append_to_json_log("Microphone", &transcript, &app_data)?;
        append_to_json_log("OPENAI RESPONSE", &gpt_response, &app_data)?;

        // Update shared state so /transcript endpoint shows the latest
        {
            let mut t = app_data.last_transcript.lock().await;
            *t = transcript;
        }
        {
            let mut g = app_data.last_gpt_response.lock().await;
            *g = gpt_response;
        }

        {
            let flag = app_data.is_recording.lock().await;
            if !*flag {
                println!("   >>> Recording loop ended after chunk.");
                break;
            }
        }
    }

    println!("   >>> Done with continuous chunk loop. is_recording = false.");
    Ok(())
}

/////////////////////////////////////////////////////////////
// record_audio_in_memory
//
// Switches between "arecord" (Linux) and "rec" (SoX on mac)
// based on MIC_BACKEND env var. Captures the WAV data to a
// Vec<u8> in memory. (No changes here.)
/////////////////////////////////////////////////////////////
async fn record_audio_in_memory(duration_sec: u32) -> Result<Vec<u8>> {
    let mic_cmd = get_mic_command(duration_sec)?;
    println!("   [DEBUG] Using mic command: {:?}", mic_cmd);

    // Spawn the chosen command via tokio::process::Command
    let mut child = Command::new(&mic_cmd[0])
        .args(&mic_cmd[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("Failed to spawn mic command")?;

    let mut output = Vec::new();

    // Read child's stdout asynchronously into output
    if let Some(mut stdout) = child.stdout.take() {
        stdout.read_to_end(&mut output).await
            .context("Reading from mic stdout failed")?;
    }

    // Wait for the process to finish
    let status = child.wait().await
        .context("Failed to .wait() on mic process")?;

    if !status.success() {
        anyhow::bail!("Mic command exited with non-zero status: {:?}", status);
    }

    Ok(output)
}

/////////////////////////////////////////////////////////////
// get_mic_command
//
// Returns the appropriate mic command + args for either
// "mac" (SoX) or "linux" (arecord), based on `MIC_BACKEND`.
/////////////////////////////////////////////////////////////
fn get_mic_command(duration_sec: u32) -> Result<Vec<String>> {
    let backend = env::var("MIC_BACKEND").unwrap_or_else(|_| "linux".to_string());

    if backend == "mac" {
        let cmd = vec![
            "rec".to_string(),
            "-q".to_string(),
            "-c".to_string(), "1".to_string(),
            "-r".to_string(), "16000".to_string(),
            "-b".to_string(), "16".to_string(),
            "-e".to_string(), "signed-integer".to_string(),
            "-t".to_string(), "wav".to_string(),
            "-".to_string(),
            "trim".to_string(), "0".to_string(), duration_sec.to_string(),
        ];
        Ok(cmd)
    } else {
        // Linux default: arecord -d <sec> -f cd -t wav -
        Ok(vec![
            "arecord".to_string(),
            "-d".to_string(), duration_sec.to_string(),
            "-f".to_string(), "cd".to_string(),
            "-t".to_string(), "wav".to_string(),
            "-".to_string(),
        ])
    }
}

/////////////////////////////////////////////////////////////
// transcribe_audio_with_whisper
//
// Sends the captured audio bytes to OpenAI Whisper API
/////////////////////////////////////////////////////////////
async fn transcribe_audio_with_whisper(audio_data: &[u8]) -> Result<String> {
    let api_key = env::var("OPENAI_API_KEY")
        .context("Must set OPENAI_API_KEY")?;
    println!("   [DEBUG] Sending {} bytes to Whisper API...", audio_data.len());

    let client = reqwest::Client::new();
    let form = reqwest::multipart::Form::new()
        .part("file",
              reqwest::multipart::Part::bytes(audio_data.to_vec())
                  .file_name("audio.wav")
                  .mime_str("audio/wav")?)
        .text("model", "whisper-1");

    let resp = client
        .post("https://api.openai.com/v1/audio/transcriptions")
        .header(AUTHORIZATION, format!("Bearer {}", api_key))
        .multipart(form)
        .send()
        .await
        .context("Failed to call Whisper API")?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("Whisper API error: {}", text);
    }

    let json_resp: serde_json::Value = resp.json().await
        .context("Failed to parse Whisper JSON")?;
    println!("   [DEBUG] Whisper API raw JSON: {:?}", json_resp);

    let transcript = json_resp["text"]
        .as_str()
        .unwrap_or("")
        .to_string();

    Ok(transcript)
}

/////////////////////////////////////////////////////////////
// summarize_with_gpt
//
// We now build a "chat" array with:
// - system message
// - up to 20 user/assistant messages from conversation_history
// - the new user chunk
//
// Then call GPT with "gpt-4o" per your code.
/////////////////////////////////////////////////////////////
async fn summarize_with_gpt(
    app_data: &web::Data<AppState>,
    latest_chunk: &str
) -> Result<String> {
    let api_key = env::var("OPENAI_API_KEY")
        .context("Must set OPENAI_API_KEY")?;
    println!("   [DEBUG] Sending transcript to GPT: {}", latest_chunk);

    let system_prompt = "You are listening in on a conversation. You will display your response on a monitor mounted on the wall, so the goal should be 50 words or less so they are not too small. If there is something said that you could provide some interesting information about, return a response. If there is nothing interesting to share, just return Listening...";

    // Gather last 20 messages
    let mut history = app_data.conversation_history.lock().await.clone();

    // We'll build a messages array for ChatCompletion
    let mut messages = Vec::new();
    // Start with system
    messages.push(serde_json::json!({
        "role": "system",
        "content": system_prompt
    }));

    // Add up to last 20 from conversation_history
    // Each item is ("user"|"assistant", content)
    // We’ll skip if empty. We'll do the last 20 items or fewer.
    let start_idx = if history.len() > 40 { history.len() - 40 } else { 0 };
    for (role, content) in &history[start_idx..] {
        let r = if role == "assistant" { "assistant" } else { "user" };
        messages.push(serde_json::json!({
            "role": r,
            "content": content
        }));
    }

    // Finally add the new chunk as a user message
    messages.push(serde_json::json!({
        "role": "user",
        "content": latest_chunk
    }));

    // Build request body
    let req_body = serde_json::json!({
        "model": "gpt-4o", // same as your code
        "messages": messages,
        "max_tokens": 100,
        "temperature": 0.7
    });

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.openai.com/v1/chat/completions")
        .header(AUTHORIZATION, format!("Bearer {}", api_key))
        .header(CONTENT_TYPE, "application/json")
        .json(&req_body)
        .send()
        .await
        .context("Failed to call ChatCompletion API")?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("ChatCompletion error: {}", text);
    }

    let json_resp: serde_json::Value = resp.json().await
        .context("Failed to parse GPT JSON")?;
    println!("   [DEBUG] GPT response raw JSON: {:?}", json_resp);

    let content = json_resp["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();

    Ok(content)
}

/////////////////////////////////////////////////////////////
// append_to_json_log
//
// Called after we get the new user chunk + GPT response
// Also broadcasts over SSE
/////////////////////////////////////////////////////////////
fn append_to_json_log(
    source: &str,
    text: &str,
    app_data: &web::Data<AppState>,
) -> Result<()> {
    let timestamp = Utc::now().to_rfc3339();
    let record = serde_json::json!({
        "timestamp": timestamp,
        "source": source,
        "text": text
    });

    let record_string = serde_json::to_string(&record)
        .context("Failed to serialize JSON record")?;

    // Append each JSON entry on its own line for simplicity
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("conversation_log.json")
        .context("Failed to open or create conversation_log.json")?;

    use std::io::Write;
    writeln!(file, "{}", record_string)
        .context("Failed to write JSON record")?;

    println!("   [DEBUG] Appended record to conversation_log.json: {}", record_string);

    // Also broadcast over SSE for real-time display
    let _ = app_data.log_sender.send(record_string.clone());

    Ok(())
}

/////////////////////////////////////////////////////////////
// conversation_log
//
// Returns the entire 'conversation_log.json' as text
/////////////////////////////////////////////////////////////
#[get("/conversation_log")]
async fn conversation_log() -> impl Responder {
    let path = "conversation_log.json";

    match std::fs::read_to_string(path) {
        Ok(contents) => {
            HttpResponse::Ok()
                .content_type("text/plain; charset=utf-8")
                .body(contents)
        }
        Err(e) => {
            HttpResponse::NotFound()
                .body(format!("Failed to read {path}: {e}"))
        }
    }
}

/////////////////////////////////////////////////////////////
// live_log_sse
//
// SSE endpoint that streams appended lines in real-time
/////////////////////////////////////////////////////////////
#[get("/live_log")]
async fn live_log_sse(app_data: web::Data<AppState>) -> HttpResponse {
    let rx = app_data.log_sender.subscribe();

    let sse_stream = BroadcastStream::new(rx).map(|res| {
        match res {
            Ok(line) => {
                let msg = format!("data: {}\n\n", line);
                Ok::<Bytes, std::io::Error>(Bytes::from(msg))
            }
            Err(_) => {
                Ok::<Bytes, std::io::Error>(Bytes::from("data:\n\n"))
            }
        }
    });

    HttpResponse::Ok()
        .content_type("text/event-stream")
        .streaming(sse_stream)
}
