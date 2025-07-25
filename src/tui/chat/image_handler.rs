use crate::api::models::BroadcastMessage;
use crate::app::AppState;
use image::ImageFormat;
use image::ImageReader;
use log::{error, info};
use std::io::{self};
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex};

/// Helper to update message and log debug info
async fn update_and_log_message(
    app_state: &Arc<Mutex<AppState>>,
    message: BroadcastMessage,
    context: &str,
) {
    let mut state = app_state.lock().await;
    log::debug!(
        "image_handler: [{}] Updating message: file_id={:?} timestamp={:?} file_name={:?} is_gif={} gif_frames={} image_preview={}",
        context,
        message.file_id,
        message.timestamp,
        message.file_name,
        message.gif_frames.is_some(),
        message.gif_frames.as_ref().map(|f| f.len()).unwrap_or(0),
        message.image_preview.is_some()
    );
    state.update_message(message);
}

/// A robust, non-blocking function to execute the chafa command.
pub async fn run_chafa(image_data: &[u8], height: u16) -> Result<String, String> {
    let size_arg = format!("--size=x{}", height);
    let args = [size_arg.as_str(), "-f", "symbols"];
    let command_str = format!("chafa {}", args.join(" "));
    info!("Executing command: {}", &command_str);

    let mut command = Command::new("chafa");
    command.args(&args);
    command.stdin(std::process::Stdio::piped());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    let mut child = command.spawn().map_err(|e| {
        error!(
            "Failed to spawn chafa command. Is 'chafa' installed and in your system's PATH? Error: {}",
            e
        );
        format!(
            "Failed to run chafa. Is it installed and in your PATH? Details: {}",
            e
        )
    })?;

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let data = image_data.to_vec();
    tokio::spawn(async move {
        stdin
            .write_all(&data)
            .await
            .expect("Failed to write to stdin");
    });

    let output = child.wait_with_output().await.map_err(|e| {
        error!("Failed to wait for chafa command: {}", e);
        format!("Failed to wait for chafa command: {}", e)
    })?;

    if output.status.success() {
        info!("Chafa command executed successfully.");
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        error!(
            "Chafa command failed!\n\
            - Status: {}\n\
            - Command: {}\n\
            - Stderr: {}\n\
            - Stdout: {}",
            output.status, command_str, stderr, stdout
        );
        Err(format!(
            "Chafa conversion failed. Stderr: {}",
            if stderr.is_empty() {
                "No error message from chafa. Check logs for more details."
            } else {
                &stderr
            }
        ))
    }
}

pub async fn convert_image_to_chafa(image_data: &[u8], height: u16) -> Result<String, String> {
    info!("Handling static image to Chafa conversion.");
    // Assume PNG/JPEG input for static images
    run_chafa(image_data, height).await
}

#[allow(dead_code)]
pub fn is_gif(data: &[u8]) -> bool {
    ImageReader::new(io::Cursor::new(data))
        .with_guessed_format()
        .map(|reader| reader.format() == Some(ImageFormat::Gif))
        .unwrap_or(false)
}

/// Processes an image message, converting it and registering for animation if it's a GIF.
pub async fn process_image_message(
    app_state: Arc<Mutex<AppState>>,
    mut message: BroadcastMessage,
    http_client: &reqwest::Client,
    height: u16,
) {
    log::debug!(
        "process_image_message: file_id={:?} timestamp={}",
        message.file_id,
        message.timestamp
    );
    let file_id = message.file_id.clone().unwrap_or_default();
    let file_name = message.file_name.clone().unwrap_or_default();

    match crate::api::file_api::download_file(
        http_client,
        &file_id,
        &file_name,
        mpsc::unbounded_channel().0,
    )
    .await
    {
        Ok(file_path) => {
            let mut file = match File::open(&file_path).await {
                Ok(f) => f,
                Err(e) => {
                    message.content = format!("[Error opening downloaded file: {}]", e);
                    update_and_log_message(&app_state, message, "download_open_error").await;
                    return;
                }
            };
            let mut image_data = Vec::new();
            if let Err(e) = file.read_to_end(&mut image_data).await {
                message.content = format!("[Error reading downloaded file: {}]", e);
                update_and_log_message(&app_state, message, "download_read_error").await;
                return;
            }

            if crate::tui::chat::gif_renderer::is_gif(&image_data) {
                match crate::tui::chat::gif_renderer::convert_gif_to_chafa_frames_and_delays(
                    &image_data,
                    height,
                )
                .await
                {
                    Ok((frames, delays)) if frames.len() > 1 => {
                        message.content = (*frames[0]).clone();
                        let frames_with_delays: Vec<(String, std::time::Duration)> = frames
                            .iter()
                            .map(|arc| (**arc).clone())
                            .zip(
                                delays
                                    .iter()
                                    .map(|d| std::time::Duration::from_millis(*d as u64)),
                            )
                            .collect();
                        message.gif_frames = Some(frames_with_delays);
                        let animation_state =
                            crate::tui::chat::gif_renderer::GifAnimationState::new(
                                frames.clone(),
                                delays.clone(),
                            );
                        let animation_state_arc =
                            std::sync::Arc::new(std::sync::Mutex::new(animation_state));
                        // TODO: Store thread handle in GifAnimationState after spawning animation thread.
                        let (frame_tx, _frame_rx) = tokio::sync::mpsc::unbounded_channel();
                        let thread_handle = crate::tui::chat::gif_renderer::spawn_gif_animation(
                            animation_state_arc.clone(),
                            frame_tx,
                        );
                        {
                            let mut state = animation_state_arc.lock().unwrap();
                            state.thread_handle = Some(thread_handle);
                        }
                        {
                            let mut state = app_state.lock().await;
                            state
                                .active_animations
                                .insert(file_id.clone(), animation_state_arc.clone());
                        }
                        // Optionally: handle frame_rx to update UI with new frames
                        update_and_log_message(&app_state, message, "gif_ok").await;
                    }
                    Ok((frames, delays)) if !frames.is_empty() => {
                        message.content = (*frames[0]).clone();
                        let frames_with_delays: Vec<(String, std::time::Duration)> = frames
                            .iter()
                            .map(|arc| (**arc).clone())
                            .zip(
                                delays
                                    .iter()
                                    .map(|d| std::time::Duration::from_millis(*d as u64)),
                            )
                            .collect();
                        message.gif_frames = Some(frames_with_delays);
                        update_and_log_message(&app_state, message, "gif_static").await;
                    }
                    Err(e) => {
                        message.content = format!("[Error converting GIF: {}]", e);
                        update_and_log_message(&app_state, message, "gif_error").await;
                    }
                    _ => {
                        message.content = "[Could not display GIF]".to_string();
                        update_and_log_message(&app_state, message, "gif_unknown").await;
                    }
                }
            } else {
                // Handle static images
                match convert_image_to_chafa(&image_data, height).await {
                    Ok(chafa_string) => {
                        message.content = chafa_string.clone();
                        message.image_preview = Some(chafa_string);
                        update_and_log_message(&app_state, message, "static_ok").await;
                    }
                    Err(e) => {
                        message.content = format!("[Error converting image: {}]", e);
                        update_and_log_message(&app_state, message, "static_error").await;
                    }
                }
            }
        }
        Err(e) => {
            // Handle error in downloading file
            message.content = format!("[Error downloading file: {}]", e);
            update_and_log_message(&app_state, message, "download_error").await;
        }
    }
}
