mod agreement;
mod api;
mod attention;
mod auth;
mod auto_sms;
mod callbacks;
mod config;
mod crypto;
mod crypto_health;
mod db_manager;
mod deny_list;
mod fromqueuemanager;
mod help;
mod macros;
mod mas_admin;
mod matrix;
mod names;
mod persistent_session;
mod retention;
mod room_purge;
mod room_commands;
mod rooms;
mod toqueuemanager;
mod userroom;
mod utils;
mod verification;
mod voip_ms;
mod web_abuse;
mod web_chat;
mod web_server;
mod web_sites;
use crate::db_manager::DbManager;
use crate::fromqueuemanager::FromQueueManager;
use crate::matrix::Matrix;
use crate::persistent_session::PersistentSession;
use crate::toqueuemanager::ToQueueManager;
use crate::verification::UserVerificationHandler;
use crate::voip_ms::VoipMs;
use crate::web_server::QueryParams;
use crate::web_server::WebServer;
use anyhow::{Context, Result};
use chrono_tz::US::Eastern;
use dotenvy::dotenv;
use std::env;
use std::process::exit;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tokio::time::timeout;
use tokio_schedule::{Job, every};
use tracing::warn;
use tracing::{error, info};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::Builder;
use tracing_appender::rolling::Rotation;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};
/// Entry point for the whole system. Call the multiple threads necessary to proper system synchronization
/// Note that the struct persistent_session should run at all time for the other threads to run properly.
/// this is because the matrix client must be synchronized for some part of the program to work.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Subcommands for the installer: run once and exit, without starting the bridge or logging
    if let Some(command) = std::env::args().nth(1) {
        return run_subcommand(&command);
    }
    // The worker guard must be kept here otherwise, they are dropped and init tracing won't work
    let (_guard1, _guard2, _guard3) = init_tracing();
    // Initialize axum crypto provider
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    if dotenv().is_ok() {
        info!("Loaded .env file");
    } else {
        info!("No .env file found, using system environment variables");
    }
    match config::check_secrets(|name| std::env::var(name).ok()) {
        Ok(warnings) => warnings.iter().for_each(|w| warn!("{w}")),
        Err(problem) => {
            error!("Refusing to start: {problem}");
            exit(1);
        }
    }

    let db_manager = DbManager::new();
    db_manager.log_db_path();
    db_manager.create_db_if_not_exist().unwrap_or_else(|err| {
        error!("Database error : {err}");
        exit(1);
    });

    /// START TEST
    // macros::add_simple_macro("bliou".to_string(), 24, "blablabla".to_string()).unwrap();
    // let test_macro = macros::get_macro_by_call_center_id("alice".to_string(), 24);
    // println!("Macro text {:?}",test_macro.unwrap().macro_text);
    // let text = macros::get_newcomer_macro_text(24);
    // println!("{:?}",text);
    // exit(0);
    /// END TEST
    // Optional test to voip.ms (if this does not work, nothing work...) For initial install only
    let voiptext = VoipMs::new();
    info!("Testing voip.ms connection for admin user");
    match timeout(Duration::from_millis(2000), voiptext.test_connection(1)).await {
        Ok(answer) => {
            if !answer.expect("Database or Network issue while testing voip.ms") {
                error!("Voip.ms not working");
            }
        }
        Err(err) => {
            error!("Timeout elapsed while testing voip.ms connection {}", err);
        }
    }

    //let args: Vec<String> = env::args().collect();

    // Channel to message incoming SMS via webserver to matrix
    let (webservertx, matrixserverrx) = mpsc::channel::<QueryParams>(32);
    let session = PersistentSession::new().await;
    let server = WebServer::new(webservertx.clone(), session.get_client().await);

    let client = session.get_client().await;
    // We make sure the device is bootstrapped, non self verified device lead to all kind of issues so we force it
    // Note that if you use only password authentification, this will not work. A matrix authentification system
    // Is now mandatory on the few clients that really work in all case eg.: element x.
    let mydevice = client
        .encryption()
        .get_own_device()
        .await
        .unwrap_or_else(|err| {
            error!("Unable to get our own device ! {err}");
            None
        });
    match mydevice {
        Some(adevice) => {
            info!(
                "Is this device self verified ? is_cross_signed_by_owner() {}",
                adevice.is_cross_signed_by_owner()
            );
            if !adevice.is_cross_signed_by_owner() {
                // Waiting for a human to approve a cross-signing reset blocks the whole bridge (no web
                // server, no queues), so it only happens when explicitly asked for.
                if matches!(
                    std::env::var("BRIDGE_WAIT_FOR_CROSS_SIGNING").unwrap_or_default().to_lowercase().as_str(),
                    "true" | "1" | "yes" | "on"
                ) {
                    warn!(
                        "This device ({}) of {} is not cross-signed yet. Step-by-step instructions to reset this account's encryption identity will be printed shortly.",
                        adevice.device_id(),
                        adevice.user_id()
                    );
                    session.sync().await.unwrap_or_else(|err| {
                        error!("Unable to sync to matrix session while cross-signing : {err}");
                        exit(1);
                    });
                } else {
                    warn!(
                        "This device ({}) of {} is not cross-signed (according to the local store). Starting normally; set BRIDGE_WAIT_FOR_CROSS_SIGNING=true to run the cross-signing reset instead.",
                        adevice.device_id(),
                        adevice.user_id()
                    );
                }
            }
        }
        None => {}
    }

    // Loud log lines when the local encryption store does not match what Synapse holds for this device
    {
        let health_client = session.get_client().await;
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await; // just after the first sync
            if !crypto_health::check(health_client.clone()).await {
                // A brand new bot is still creating its cross-signing keys: look again once that is done
                tokio::time::sleep(Duration::from_secs(60)).await;
                crypto_health::check(health_client.clone()).await;
            }
            // A device of the bridge that is not cross-signed blocks respondents in Element X: keep looking
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
                crypto_health::check_own_devices(&health_client).await;
            }
        });
    }

    // Warn about missing admin access and remove the bot's rate limits when possible
    tokio::spawn(mas_admin::startup_check(session.get_client().await));

    let matrix = Matrix::new(Some(matrixserverrx), session.clone()).await;
    let fromqueuemanager = FromQueueManager::new(webservertx.clone()).await;
    let toqueuemanager = ToQueueManager {};
    let sync_matrix = Matrix::new(None, session.clone()).await;
    //sync_matrix.session.sync().await?; // Endless loop ! Useful for testing without starting the other threads
    let client = session.clone().get_client().await;
    let client2 = session.clone().get_client().await;
    // Web chat: purge rooms left from a previous run once the first sync is likely done, then
    // end idle sessions forever
    let web_client = session.clone().get_client().await;
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(45)).await;
        web_chat::purge_orphan_rooms(&web_client).await;
        web_chat::run_cleanup(web_client).await;
    });
    // Reminders for conversations nobody has answered yet
    tokio::spawn(attention::run(session.clone().get_client().await));
    // SMS conversations expire too (SMS_IDLE_RETENTION_DAYS)
    tokio::spawn(retention::run(session.clone().get_client().await));
    let verify_new_users_twice_every_minute = every(30).seconds().perform(move || {
        let client = client.clone(); // clone inside the closure call

        async move {
            tracing::debug!("Checking for new users to verify");

            let user_verification = UserVerificationHandler::new(client);

            if let Err(err) = user_verification.send_verification_messages(true).await {
                error!("Unable to send verification message: {err}");
            }
        }
    });
    let verify_device_reminder_every_day_at_noon_eastern = every(1)
        .day()
        .at(12, 0, 00)
        .in_timezone(&Eastern)
        .perform(move || {
            let client = client2.clone();
            async move {
                let user_verification = UserVerificationHandler::new(client.clone());
                info!("Daily reminder for users who have yet not registered all devices.");
                user_verification
                    .send_verification_messages(false)
                    .await
                    .unwrap_or_else(|err| {
                        error!("Unable to send verification message : {err}");
                    });
                // Volunteers who have not signed the confidentiality agreement get it again (reminders only)
                let users = db_manager::DbManager::new().get_all_respondant_user_id(false).ok().flatten().unwrap_or_default();
                let sent = agreement::send_to_unsigned(&client, users).await;
                info!("Daily reminder: confidentiality agreement sent to {sent} volunteer(s) who have not signed it");
            }
        });

    // Tasks that should not crash or are not essential
    info!("Starting crons tasks");
    tokio::spawn(async move {
        tokio::join!(
            verify_new_users_twice_every_minute,
            verify_device_reminder_every_day_at_noon_eastern
        );
    });
    // Replaced let _result = tokio::join!(
    info!("Starting main tasks");
    let result = tokio::try_join!(
        async {
            matrix
                .listen_sms_message()
                .await
                .context("matrix.listen_sms_message (SMS listener) task failed")
        },
        async {
            server
                .bind_server()
                .await
                .context("server.bind_server (web server) task failed")
        },
        async {
            sync_matrix
                .session
                .sync()
                .await
                .context("session.sync (Matrix sync loop) task failed")
        },
        async {
            fromqueuemanager
                .loop_queue()
                .await
                .context("fromqueuemanager.loop_queue task failed")
        },
        async {
            toqueuemanager
                .loop_queue()
                .await
                .context("toqueuemanager.loop_queue task failed")
        } // Deprecated user_verification.test_verification() // Verifications can be interrupted by other tasks (unverified)!! fromqueue tested with errors
    );

    info!("Waiting for errors");

    match result {
        Ok(_) => return Ok(()),
        Err(err) => {
            error!("Error, one of the essential tasks failed: {err:?}");
            exit(1);
        }
    }
}

///
/// `admin-token` prints the admin API token (privilege 101, creates the database if needed);
/// `regenerate-admin-token` replaces it first. The token goes to stdout only, never to a log.
fn run_subcommand(command: &str) -> anyhow::Result<()> {
    let regenerate = match command {
        "admin-token" => false,
        "regenerate-admin-token" => true,
        _ => {
            eprintln!("usage: switchboard-chat [admin-token | regenerate-admin-token]");
            exit(2);
        }
    };
    let _ = dotenv();
    if let Err(problem) = config::check_secrets(|name| std::env::var(name).ok()) {
        eprintln!("Refusing to run: {problem}");
        exit(1);
    }
    let db_manager = DbManager::new();
    db_manager.create_db_if_not_exist()?;
    let (token, _) = db_manager.create_or_update_call_center("ADMIN", "ADMIN", 1234561234, 101, regenerate)?;
    println!("{token}");
    Ok(())
}

fn init_tracing() -> (WorkerGuard, WorkerGuard, WorkerGuard) {
    // Define the 2 logs files separated, define how they are rolled
    let data_dir = env::var("APP_DATA_DIR").unwrap_or_else(|_| "./data".to_string());
    let log_dir = data_dir + "/logs";
    // Logs are confidential: the directory is created owner-only (0700), which keeps other
    // users out of the files inside. tracing-appender has no API to set a file mode, so the
    // files themselves get the process umask (a umask of 077 in the container would make them 0600).
    {
        use std::os::unix::fs::DirBuilderExt;
        // On failure the appender below reports the problem
        let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&log_dir);
    }
    let app_log = Builder::new()
        .rotation(Rotation::DAILY) // rotate every day
        .max_log_files(7) // keep 7 most recent files
        .filename_prefix("app") // file prefix (will generate app.YYYY-MM-DD.log)
        .filename_suffix("log") // file suffix
        .build(&log_dir) // directory where logs go
        .expect("failed to build rolling appender");

    let (app_writer, _guard1) = tracing_appender::non_blocking(app_log);
    let app_layer = fmt::layer().with_writer(app_writer);

    let warn_log = Builder::new()
        .rotation(Rotation::DAILY) // rotate every day
        .max_log_files(7) // keep 7 most recent files
        .filename_prefix("warn") // file prefix (will generate app.YYYY-MM-DD.log)
        .filename_suffix("log") // file suffix
        .build(&log_dir) // directory where logs go
        .expect("failed to build rolling appender");
    let (warn_writer, _guard2) = tracing_appender::non_blocking(warn_log);
    let warn_layer = fmt::layer().with_writer(warn_writer);

    let error_log = Builder::new()
        .rotation(Rotation::DAILY) // rotate every day
        .max_log_files(7) // keep 7 most recent files
        .filename_prefix("error") // file prefix (will generate app.YYYY-MM-DD.log)
        .filename_suffix("log") // file suffix
        .build(&log_dir) // directory where logs go
        .expect("failed to build rolling appender");
    let (error_writer, _guard3) = tracing_appender::non_blocking(error_log);
    let error_layer = fmt::layer().with_writer(error_writer);

    let debug_everything = false;
    // Keep the pretty colors (green = info, red = error), only display the current crate
    let stdout_layer = fmt::layer().with_ansi(true).pretty();

    let stdout_layer = if debug_everything {
        stdout_layer.boxed() // Box to make types uniform
    } else {
        stdout_layer
            .with_filter(EnvFilter::new("switchboard_chat=debug,matrix_sdk_crypto=info"))
            .boxed() // Box to make types uniform
    };
    // Apply the tracing suscriber with non ansi log files for readability.
    tracing_subscriber::registry()
        .with(stdout_layer) // terminal only
        .with(
            app_layer
                .with_ansi(false)
                .with_filter(EnvFilter::new("switchboard_chat=info"))
                .with_filter(LevelFilter::INFO),
        )
        .with(
            warn_layer
                .with_ansi(false)
                .with_filter(EnvFilter::new("switchboard_chat=warn"))
                .with_filter(LevelFilter::WARN),
        )
        .with(
            error_layer
                .with_ansi(false)
                .with_filter(EnvFilter::new("switchboard_chat=error"))
                .with_filter(LevelFilter::ERROR),
            //.with_filter(EnvFilter::new("switchboard_chat=error")),
        )
        .init();

    info!("Server Started");
    (_guard1, _guard2, _guard3)
}
