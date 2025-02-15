/////////////////////////////////////////////////////////////
// src/main.rs
//
// A single Actix Web server that records 5s of audio 
// in memory using `arecord` (async), then sends that WAV 
// data to Whisper & GPT.
//
// This works on Linux systems with ALSA installed (arecord).
// On macOS, arecord won't run natively without special 
// emulation + ALSA support, so the microphone capture 
// likely will not work.
/////////////////////////////////////////////////////////////

use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use std::env;
use std::sync::Arc;
use std::fs;

use tokio::sync::Mutex as AsyncMutex;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use anyhow::{Context, Result};
use serde::Serialize;
use std::process::Stdio;

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
//   1) Capture 5s of audio via arecord -> in-memory
//   2) Transcribe via Whisper
//   3) Summarize via GPT
//   4) Update the shared transcript/gpt fields
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

    HttpResponse::Ok().body("Recording started in memory for 5s...")
}

/////////////////////////////////////////////////////////////
// POST /stop_recording
//
// Sets is_recording = false. We do NOT forcibly kill arecord 
// if it's still running (because -d 5 auto-stops after 5s).
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

    // Initialize shared state
    let app_state = web::Data::new(AppState {
        is_recording: Arc::new(AsyncMutex::new(false)),
        last_transcript: Arc::new(AsyncMutex::new(String::new())),
        last_gpt_response: Arc::new(AsyncMutex::new(String::new())),
    });

    // Launch Actix Web
    HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .service(index)
            .service(get_transcript)
            .service(start_recording)
            .service(stop_recording)
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}

/////////////////////////////////////////////////////////////
// record_and_process_audio
//
// 1) record_audio_in_memory(5) => in-memory WAV data
// 2) transcribe_audio_with_whisper => get transcript
// 3) summarize_with_gpt => short summary
// 4) store results in app_data
/////////////////////////////////////////////////////////////
async fn record_and_process_audio(app_data: web::Data<AppState>) -> Result<()> {
    println!("   >>> Starting 5s in-memory recording via tokio::process::Command...");

    // Record audio
    let audio_data = record_audio_in_memory(5).await?;
    println!("   >>> Recording complete, {} bytes captured.", audio_data.len());

    // Whisper
    println!("   >>> Transcribing with Whisper...");
    let transcript = transcribe_audio_with_whisper(&audio_data).await?;
    println!("   >>> Transcript: {}", transcript);

    // GPT Summarize
    println!("   >>> Summarizing with GPT...");
    let gpt_response = summarize_with_gpt(&transcript).await?;
    println!("   >>> GPT response: {}", gpt_response);

    // Update Shared State
    {
        let mut t = app_data.last_transcript.lock().await;
        *t = transcript;
    }
    {
        let mut g = app_data.last_gpt_response.lock().await;
        *g = gpt_response;
    }
    {
        let mut r = app_data.is_recording.lock().await;
        *r = false;
    }
    println!("   >>> Done. is_recording = false.");

    Ok(())
}

/////////////////////////////////////////////////////////////
// record_audio_in_memory
//
// Runs `arecord -d <duration_sec> -f cd -t wav -` and 
// captures stdout into a Vec<u8>.
/////////////////////////////////////////////////////////////
async fn record_audio_in_memory(duration_sec: u32) -> Result<Vec<u8>> {
    // Spawn arecord via tokio::process::Command
    let mut child = Command::new("arecord")
        .args(&[
            "-d", &duration_sec.to_string(),
            "-f", "cd",
            "-t", "wav",
            "-", // write to stdout
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("Failed to spawn 'arecord' via tokio::process")?;

    let mut output = Vec::new();

    // Read child's stdout asynchronously into output
    if let Some(mut stdout) = child.stdout.take() {
        stdout.read_to_end(&mut output).await
            .context("Reading from 'arecord' stdout failed")?;
    }

    // Wait for arecord to finish
    let status = child.wait().await
        .context("Failed to .wait() on arecord child process")?;

    if !status.success() {
        anyhow::bail!("'arecord' exited with non-zero status: {:?}", status);
    }

    Ok(output)
}

/////////////////////////////////////////////////////////////
// transcribe_audio_with_whisper
//
// Sends the captured audio bytes to OpenAI Whisper API
/////////////////////////////////////////////////////////////
async fn transcribe_audio_with_whisper(audio_data: &[u8]) -> Result<String> {
    let api_key = env::var("OPENAI_API_KEY")
        .context("Must set OPENAI_API_KEY")?;

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
    let transcript = json_resp["text"]
        .as_str()
        .unwrap_or("")
        .to_string();

    Ok(transcript)
}

/////////////////////////////////////////////////////////////
// summarize_with_gpt
//
// Sends the transcription to GPT-3.5 or GPT-4 for summarizing
/////////////////////////////////////////////////////////////
async fn summarize_with_gpt(transcript: &str) -> Result<String> {
    let api_key = env::var("OPENAI_API_KEY")
        .context("Must set OPENAI_API_KEY")?;

    let system_prompt = "You are a helpful AI. Summarize the user's speech briefly.";
    let req_body = serde_json::json!({
        "model": "gpt-3.5-turbo",
        "messages": [
            { "role": "system", "content": system_prompt },
            { "role": "user",   "content": transcript }
        ],
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
    let content = json_resp["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();

    Ok(content)
}
