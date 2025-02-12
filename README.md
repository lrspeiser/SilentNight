# Silent Night

## Overview
This project is a **real-time AI-powered conversation listener** that runs on a **Raspberry Pi 5**, listens to speech using a microphone, transcribes it using OpenAI's Whisper model, and displays insightful responses on an HDMI-connected monitor using OpenAI's GPT-4. If there is nothing interesting to display, it will simply show *"Listening..."*.

## Features
- **Real-time Speech-to-Text**: Uses OpenAI's Whisper model to transcribe audio.
- **Context-Aware Responses**: Utilizes GPT-4 to generate brief and relevant responses.
- **Visual Display**: Displays responses on an HDMI-connected screen.
- **Streaming Output**: Uses Server-Sent Events (SSE) to display responses as they arrive.

## Hardware Requirements
- **Raspberry Pi 5**
- **Microphone** (e.g., Seeed Studio ReSpeaker Mic Array v2.0)
- **HDMI Monitor**
- **Internet Connection** (for OpenAI API calls)

## Software Requirements
- **Rust** (latest stable version)
- **Tokio** (asynchronous runtime)
- **Reqwest** (HTTP client for API calls)
- **serde_json** (JSON handling)
- **crossterm** (console output management)

## Installation
### 1. Clone the Repository
```sh
git clone https://github.com/your-username/conversation-listener.git
cd conversation-listener
```

### 2. Install Rust
Ensure you have Rust installed. If not, install it with:
```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 3. Set Up Environment Variables
You need an OpenAI API key. Set it in your environment:
```sh
export OPENAI_API_KEY="your_openai_api_key"
```

### 4. Run the Application
```sh
cargo run
```

## How It Works
1. The program **checks for the OpenAI API key**.
2. It verifies that a recorded audio file (`output.wav`) exists.
3. It **transcribes** the audio using OpenAI's Whisper API.
4. It sends the transcribed text to **GPT-4**, along with a system prompt.
5. The response is **streamed** and displayed on an HDMI-connected screen.
6. If nothing insightful is found, the display simply shows *"Listening..."*.

## Code Breakdown
- **`transcribe_audio_with_whisper()`**: Uploads the recorded WAV file to OpenAI Whisper for transcription.
- **`stream_gpt_response()`**: Sends the transcribed text to GPT-4 and streams the response to the terminal and screen.

## Example Output
```sh
🚀 Starting the 'Conversation Listener' demo...
🔑 Checking for OPENAI_API_KEY...
✅ Found OPENAI_API_KEY.
💾 Checking for WAV file `output.wav`...
📝 Transcribing `output.wav` with OpenAI Whisper...
📜 Transcription: "The Eiffel Tower was built in 1889."
🤖 Sending system and user prompts to GPT-4 with streaming...
💡 AI: "Did you know? The Eiffel Tower was originally supposed to be dismantled after 20 years, but it remained due to its usefulness as a radio tower!"
```

## Troubleshooting
### Missing API Key
Ensure you have set the `OPENAI_API_KEY` environment variable:
```sh
export OPENAI_API_KEY="your_openai_api_key"
```

### Missing `output.wav`
Record an audio file and place it in the project directory:
```sh
arecord -d 10 -f cd output.wav
```

### API Errors
Check your OpenAI API key and internet connection. View logs for details.

## Future Enhancements
- **Real-time audio capture** instead of pre-recorded WAV files.
- **Integration with a visual display library** for richer UI elements.
- **Support for multiple microphones** for improved speech recognition.

## License
This project is open-source and licensed under the MIT License.

## Author
Created by **Leonard Speiser** - Contributions Welcome!

