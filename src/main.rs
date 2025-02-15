/////////////////////////////////////////////////////////////
// main.rs
//
// Rust + Actix-Web example that binds to port 80.
// Replit's deployment feature sometimes insists on port 80
// for successful deployment. 
//
// NOTE: The "arecord" calls will not work on Replit, but 
// this code shows how to open port 80 and avoid the 
// deployment port error.
//
// Project Structure:
//   - Cargo.toml (with actix-web, tokio, etc.)
//   - src/main.rs (this file)
//   - static/index.html (HTML to serve on GET /)
/////////////////////////////////////////////////////////////

use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use std::process::Command;
use std::fs;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

// WAV filename
const WAV_FILENAME: &str = "output.wav";

/////////////////////////////////////////////////////////////
// AppState
//
// Holds a shared boolean to indicate if recording is active.
/////////////////////////////////////////////////////////////
struct AppState {
    is_recording: Arc<AsyncMutex<bool>>,
}

/////////////////////////////////////////////////////////////
// GET /
//
// Serves a basic index.html from the "static" directory.
/////////////////////////////////////////////////////////////
#[get("/")]
async fn index() -> impl Responder {
    println!("▶ GET / - Serving index.html...");

    // Attempt to read the HTML file
    let html = fs::read_to_string("static/index.html")
        .unwrap_or_else(|_| "<h1>Index file not found!</h1>".to_string());

    HttpResponse::Ok()
        .content_type("text/html")
        .body(html)
}

/////////////////////////////////////////////////////////////
// POST /start_recording
//
// Sets is_recording = true, spawns "arecord" command.
//
// WARNING: Replit deploy does NOT allow actual microphone 
// access. The arecord call will do nothing in Replit.
/////////////////////////////////////////////////////////////
#[post("/start_recording")]
async fn start_recording(data: web::Data<AppState>) -> impl Responder {
    println!("▶ POST /start_recording - checking if already recording...");

    // Lock the boolean
    let mut rec_guard = data.is_recording.lock().await;
    if *rec_guard {
        println!("   Already recording!");
        return HttpResponse::Ok().body("Already recording");
    }

    // Mark as recording
    *rec_guard = true;
    println!("   Marked as recording. Spawning arecord...");

    // Spawn a background command (won't really work in Replit)
    tokio::spawn(async {
        let _ = Command::new("arecord")
            .args(&["-d", "5", "-f", "cd", WAV_FILENAME])
            .status();
    });

    HttpResponse::Ok().body("Recording started")
}

/////////////////////////////////////////////////////////////
// POST /stop_recording
//
// Sets is_recording = false.
/////////////////////////////////////////////////////////////
#[post("/stop_recording")]
async fn stop_recording(data: web::Data<AppState>) -> impl Responder {
    println!("▶ POST /stop_recording - setting is_recording = false");
    let mut rec_guard = data.is_recording.lock().await;
    *rec_guard = false;
    HttpResponse::Ok().body("Recording stopped")
}

/////////////////////////////////////////////////////////////
// MAIN - entry point
//
// Binds explicitly to port 80 for Replit Deployment.
/////////////////////////////////////////////////////////////
#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Replit's new deployment feature can require port 80:
    let port: u16 = 80;
    println!("============================================================");
    println!("🚀 Starting Actix Web server on port {}", port);
    println!("    Replit Deployment typically requires port 80.");
    println!("============================================================");

    // Create shared state
    let app_state = web::Data::new(AppState {
        is_recording: Arc::new(AsyncMutex::new(false)),
    });

    // Start the server on 0.0.0.0:80
    HttpServer::new(move || {
        App::new()
            .app_data(app_state.clone())
            .service(index)
            .service(start_recording)
            .service(stop_recording)
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
