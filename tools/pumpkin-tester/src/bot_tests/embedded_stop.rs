use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};

use pumpkin_config::TelemetryConfig;
use pumpkin_core::{
    PumpkinServer, STOP_INTERRUPT,
    data::VanillaData,
    net::{PacketRateLimiter, java::pending::PendingConnection},
    serve_java_connection,
};
use tokio::time::timeout;

use crate::{bot::HeadlessBot, harness};

const STOP_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn run_embedded_stop_test(summary_lines: &mut Vec<String>) -> Result<(), String> {
    let test_start = Instant::now();
    let temp_dir = tempfile::tempdir()
        .map_err(|err| format!("Failed to create embedded world directory: {err}"))?;
    let (basic_config, mut advanced_config) = harness::test_configs(&temp_dir);
    advanced_config.networking.java.enabled = false;
    advanced_config.player_data.save_player_data = true;
    let telemetry_config = TelemetryConfig {
        enabled: false,
        ..Default::default()
    };

    let pumpkin_server = PumpkinServer::new(
        basic_config,
        advanced_config,
        telemetry_config,
        VanillaData::default(),
        Vec::new(),
        false,
    )
    .await
    .map_err(|err| format!("Failed to initialize embedded server: {err}"))?;
    let server = pumpkin_server.server.clone();
    harness::prepare_spawn_chunk(&server).await;

    let (client, transport) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(transport);
    let (client_read, client_write) = tokio::io::split(client);
    let pending = PendingConnection::from_transport(
        Box::new(server_read),
        Box::new(server_write),
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        1,
        PacketRateLimiter::from_config(&server.advanced_config.networking.java.packet_limiter),
        Arc::downgrade(&server),
    );
    let connection = tokio::spawn(serve_java_connection(server.clone(), pending));

    let session = async {
        let joined = HeadlessBot::join(
            Box::new(client_read),
            Box::new(client_write),
            0,
            "EmbeddedBot",
        )
        .await;
        let result = async {
            let mut bot = joined?;
            bot.set_held_item(6).await?;
            for _ in 0..crate::bot::MAX_POLL_TICKS {
                if let Some(player) = server.get_all_players().first()
                    && player.inventory.get_selected_slot() == 6
                {
                    return Ok::<_, String>((bot, player.gameprofile.id));
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Err("EmbeddedBot did not select hotbar slot 6".into())
        }
        .await;
        server.stop();
        result
    };
    let ((), joined) = timeout(STOP_TIMEOUT, async {
        tokio::join!(pumpkin_server.start(), session)
    })
    .await
    .map_err(|_| "Embedded server did not finish stopping".to_string())?;
    let (_bot, player_id) = joined?;

    timeout(STOP_TIMEOUT, connection)
        .await
        .map_err(|_| "Embedded connection did not end after stop".to_string())?
        .map_err(|err| format!("Embedded connection task failed: {err}"))?;

    if STOP_INTERRUPT.is_cancelled() {
        return Err("Stopping the embedded server stopped the whole process".to_string());
    }

    let data = server
        .player_data_storage
        .load_data(&player_id)
        .map_err(|err| format!("Saved player data could not be loaded: {err}"))?
        .ok_or("Embedded server did not save player data")?;
    if data.get_int("SelectedItemSlot") != Some(6) {
        return Err("Embedded shutdown did not persist selected hotbar slot 6".into());
    }

    summary_lines.push(format!(
        "[PASS] bot:embedded_stop ({}ms)",
        test_start.elapsed().as_millis()
    ));
    Ok(())
}
