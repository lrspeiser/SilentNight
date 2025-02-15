use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use std::process::Command;
use std::sync::Mutex;
use std::fs;
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;

struct AppState {
    is_recording: Arc<AsyncMutex<bool>>,
}

#[get("/")]
async fn index() -> impl Responder {
    let html = fs::read_to_string("static/index.html").unwrap();
    HttpResponse::Ok().content_type("text/html").body(html)
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
            .args(&["-d", "5", "-f", "cd", "output.wav"])
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

#[actix_web::main]
async fn main() -> std::io::Result<()> {
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
    .bind("0.0.0.0:8080")?
    .run()
    .await
}
