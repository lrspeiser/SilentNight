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

// We'll transcribe this file
const WAV_FILENAME: &str = "output.wav";

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    println!("🚀 Starting the 'Conversation Listener' demo...");

    // 1) Check for OpenAI API key
    println!("🔑 Checking for OPENAI_API_KEY...");
    let openai_api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) => {
            println!("✅ Found OPENAI_API_KEY.");
            k
        }
        Err(_) => {
            eprintln!("❌ OPENAI_API_KEY not set. Please add it to environment or Replit Secrets.");
            return;
        }
    };

    // 2) Ensure the WAV file exists
    println!("💾 Checking for WAV file `{}`...", WAV_FILENAME);
    if !std::path::Path::new(WAV_FILENAME).exists() {
        eprintln!("❌ WAV file `{}` does not exist. Please upload it.", WAV_FILENAME);
        return;
    }

    // 3) Transcribe the WAV file with Whisper
    println!("📝 Transcribing `{}` with OpenAI Whisper...", WAV_FILENAME);
    let transcribed_text = match transcribe_audio_with_whisper(WAV_FILENAME, &openai_api_key).await {
        Ok(t) => {
            println!("📜 Transcription: {}", t);
            t
        }
        Err(e) => {
            eprintln!("❌ Error transcribing audio: {:?}", e);
            return;
        }
    };

    // 4) Prepare the system and user messages for GPT-4
    let system_prompt = "You are listening in on a conversation. If there is something said that you could provide some interesting information about, return a response. If there is nothing interesting to share, just return Listening...";
    let user_content = transcribed_text;

    // 5) Call GPT-4 with streaming
    println!("🤖 Sending system and user prompts to GPT-4 with streaming...");
    stream_gpt_response(system_prompt, &user_content, &openai_api_key).await;

    println!("🏁 Done. Exiting now.");
}

/// Uploads a WAV file to OpenAI Whisper (/v1/audio/transcriptions) and returns the recognized text.
async fn transcribe_audio_with_whisper(
    filepath: &str,
    openai_api_key: &str
) -> Result<String, Box<dyn std::error::Error>> {
    let audio_data = fs::read(filepath)?;
    println!("📦 WAV file size: {} bytes", audio_data.len());

    let part = reqwest::multipart::Part::bytes(audio_data)
        .file_name("audio.wav")
        .mime_str("audio/wav")?;

    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", "whisper-1");

    let url = "https://api.openai.com/v1/audio/transcriptions";
    let client = Client::new();
    let res = client
        .post(url)
        .header("Authorization", format!("Bearer {}", openai_api_key))
        .multipart(form)
        .send()
        .await?;

    if !res.status().is_success() {
        let status = res.status();
        let err_text = res.text().await.unwrap_or_default();
        return Err(format!("Whisper API error - Status: {} | Details: {}", status, err_text).into());
    }

    let json_response: serde_json::Value = res.json().await?;
    if let Some(text) = json_response["text"].as_str() {
        Ok(text.to_string())
    } else {
        Err("No 'text' field in Whisper response".into())
    }
}

/// Streams GPT-4’s response via SSE, printing tokens as they arrive,
/// then printing the entire final assembled response at the end.
async fn stream_gpt_response(
    system_prompt: &str,
    user_message: &str,
    openai_api_key: &str
) {
    // Build ChatCompletion payload
    let payload = serde_json::json!({
        "model": "gpt-4",
        "stream": true,
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user",   "content": user_message}
        ]
    });

    println!("🔧 GPT-4 request payload:\n{:#?}", payload);

    let client = Client::new();
    let response = match client
        .post("https://api.openai.com/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", openai_api_key))
        .json(&payload)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                eprintln!("❌ GPT-4 request failed with status: {}", resp.status());
                return;
            }
            resp
        }
        Err(err) => {
            eprintln!("❌ Error sending GPT-4 request: {:?}", err);
            return;
        }
    };

    // (Optional) Clear the console
    let _ = execute!(stdout(), Clear(ClearType::All));
    print!("💡 AI: ");
    let _ = io::stdout().flush();

    // We'll store partial tokens here so we can show a final result
    let mut final_answer = String::new();

    println!("\n🔄 Starting to read GPT-4's streaming response...\n");
    let mut stream = response.bytes_stream();

    while let Some(chunk_result) = stream.next().await {
        match chunk_result {
            Ok(chunk) => {
                let chunk_str = String::from_utf8_lossy(&chunk);

                // Each chunk can contain multiple lines
                for line in chunk_str.split('\n').map(|l| l.trim()).filter(|l| !l.is_empty()) {
                    // SSE lines usually start with "data: "
                    if let Some(json_str) = line.strip_prefix("data: ") {
                        if json_str.trim() == "[DONE]" {
                            // Stream ended
                            println!("\n🔚 [DONE] received. Stopping.");
                            println!("\n======== Final GPT-4 Answer ========");
                            println!("{}", final_answer);
                            return;
                        }

                        // Attempt to parse SSE JSON
                        if let Ok(json) = serde_json::from_str::<Value>(json_str) {
                            if let Some(content) = json["choices"]
                                .get(0)
                                .and_then(|c| c["delta"]["content"].as_str())
                            {
                                // Print the partial content in real-time
                                print!("{}", content);
                                let _ = io::stdout().flush();

                                // Accumulate into final_answer
                                final_answer.push_str(content);
                            }
                        }
                    }
                }
            }
            Err(err) => {
                eprintln!("❌ Error reading SSE chunk: {:?}", err);
            }
        }
    }

    // If we exit the loop naturally (no "[DONE]"?), just print final answer
    println!("\n✅ Done streaming GPT-4 response (no [DONE] marker).");
    println!("\n======== Final GPT-4 Answer ========");
    println!("{}", final_answer);
}
