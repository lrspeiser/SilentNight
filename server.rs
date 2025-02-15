/////////////////////////////////////////////////////////////
// server.rs
//
// Revised Rust + Actix-Web server that uses a default port 
// from an environment variable or falls back to 8080 if not 
// set. It saves the 'output.wav' file to local disk on your 
// Raspberry Pi whenever the /start_recording endpoint is 
// called.
//
// Run with:
//   cargo run
//
// Or set a custom port:
//   PORT=3000 cargo run
//
// If you want to bind to port 80 on a Pi, set PORT=80 and
// run with sudo (if necessary), e.g.:
//   sudo PORT=80 cargo run
/////////////////////////////////////////////////////////////

use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use std::{env, process::Command, fs};
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

/////////////////////////////////////////////////////////////
// Shared Application State
//
// Tracks whether we are currently recording via arecord.
/////////////////////////////////////////////////////////////
struct AppState {
    is_recording: Arc<AsyncMutex<bool>>,
}

/////////////////////////////////////////////////////////////
// GET /
//
// Serves the file "static/index.html" to the browser.
/////////////////////////////////////////////////////////////
#[get("/")]
async fn index() -> impl Responder {
    println!("▶ GET /  => Serving 'static/index.html'...");

    // Attempt to read our static HTML from disk
    match fs::read_to_string("static/index.html") {
        Ok(html) => {
            println!("   Successfully read 'static/index.html'.");
            HttpResponse::Ok().content_type("text/html").body(html)
        }
        Err(err) => {
            println!("   ERROR: Could not read 'static/index.html': {}", err);
            HttpResponse::NotFound().body("<h1>index.html not found</h1>")
        }
    }
}

/////////////////////////////////////////////////////////////
// POST /start_recording
//
// 1. Checks if we are already recording; if yes, do nothing.
// 2. Otherwise, sets the flag and spawns an 'arecord' 
//    command that records for 5 seconds to "output.wav".
/////////////////////////////////////////////////////////////
#[post("/start_recording")]
async fn start_recording(data: web::Data<AppState>) -> impl Responder {
    println!("▶ POST /start_recording - Checking if already recording...");

    let mut rec_guard = data.is_recording.lock().await;
    if *rec_guard {
        println!("   Already recording. Returning early...");
        return HttpResponse::Ok().body("Already recording");
    }

    *rec_guard = true;
    println!("   Not currently recording; now setting is_recording = true.");

    // Spawn the arecord command in a background task
    println!("   Spawning 'arecord' to record audio for 5s to 'output.wav'...");
    tokio::spawn(async {
        println!("   arecord: starting...");
        let status = Command::new("arecord")
            .args(&["-d", "5", "-f", "cd", "output.wav"])
            .status();

        match status {
            Ok(s) => {
                println!("   arecord finished successfully with status: {:?}", s);
            },
            Err(e) => {
                println!("   arecord failed to run. Error: {:?}", e);
            },
        }

        // Check file size by reading the metadata of "output.wav".
        // This uses tokio::fs::metadata for async file operations.
        match tokio::fs::metadata("output.wav").await {
            Ok(meta) => {
                let size_in_bytes = meta.len();
                // Convert bytes to kilobytes (1 KB = 1024 bytes)
                let size_in_kb = size_in_bytes as f64 / 1024.0;
                println!("   'output.wav' file size: {:.2} KB ({} bytes)", 
                         size_in_kb, size_in_bytes);
            },
            Err(e) => {
                println!("   Failed to get file metadata for 'output.wav': {:?}", e);
            },
        }

        println!("   Finished writing to 'output.wav'.");
    });

    HttpResponse::Ok().body("Recording started")
}



/////////////////////////////////////////////////////////////
// POST /stop_recording
//
// Sets the is_recording flag to false.
// NOTE: We do *not* forcibly kill the 'arecord' process 
// here. The '-d 5' argument to arecord automatically stops 
// after 5 seconds.
/////////////////////////////////////////////////////////////
#[post("/stop_recording")]
async fn stop_recording(data: web::Data<AppState>) -> impl Responder {
    println!("▶ POST /stop_recording");

    let mut recording_flag = data.is_recording.lock().await;
    *recording_flag = false;
    println!("   is_recording set to false.");

    HttpResponse::Ok().body("Recording stopped")
}

/////////////////////////////////////////////////////////////
// MAIN - Actix Web Entry Point
//
// Reads a "PORT" env variable if present, otherwise defaults 
// to 8080. Binds to "0.0.0.0:<PORT>," which is suitable for 
// running on a Raspberry Pi or other local machines.
/////////////////////////////////////////////////////////////
#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Attempt to read the PORT from environment, or fallback
    let default_port = String::from("8080");
    let port = env::var("PORT").unwrap_or(default_port);
    let port: u16 = port.parse().unwrap_or(8080);

    println!("===========================================");
    println!("🚀 Starting Actix-Web server on port {}...", port);
    println!("   Serving 'static/index.html' at GET /");
    println!("   Recording to 'output.wav' at POST /start_recording");
    println!("===========================================");

    // Create our shared state
    let app_state = web::Data::new(AppState {
        is_recording: Arc::new(AsyncMutex::new(false)),
    });

    // Construct and run the HTTP server
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
