use reqwest::Client;
use tokio_stream::StreamExt;
use serde_json::Value;
use std::io::{self, Write};
use crossterm::{
    execute,
    terminal::{Clear, ClearType},
};
use std::fs;
use std::io::stdout;
use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use std::process::Command;

const WAV_FILENAME: &str = "output.wav";

struct AppState {
    is_recording: Arc<AsyncMutex<bool>>,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    println!("🚀 Starting the 'Conversation Listener' demo...");

    let app_state = web::Data::new(AppState {
        is_recording: Arc::new(AsyncMutex::new(false)),
    });

    HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .service(index)
            .service(start_recording)
            .service(stop_recording)
    })
    .bind(("0.0.0.0", 8080))?
    .run()
    .await
    .expect("Server failed to start");
}

#[get("/")]
async fn index() -> impl Responder {
    HttpResponse::Ok().body("Service is running")
}

#[post("/start_recording")]
async fn start_recording(data: web::Data<AppState>) -> impl Responder {
    let mut recording = data.is_recording.lock().await;
    if *recording {
        return HttpResponse::Ok().body("Already recording");
    }
    *recording = true;

    tokio::spawn(async {
        let _ = Command::new("arecord")
            .args(&["-d", "5", "-f", "cd", WAV_FILENAME])
            .status();
    });

    HttpResponse::Ok().body("Recording started")
}

#[post("/stop_recording")]
async fn stop_recording(data: web::Data<AppState>) -> impl Responder {
    let mut recording = data.is_recording.lock().await;
    *recording = false;
    HttpResponse::Ok().body("Recording stopped")
}

