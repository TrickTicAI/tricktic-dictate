use metrix_writer_api::{app, desktop_host, self_hosted, ProviderConfiguration};
use std::{io::Write, net::SocketAddr, path::PathBuf};
use tracing_subscriber::EnvFilter;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if let [arg, path] = args.as_slice() {
        if arg == "--init-owner-key" {
            self_hosted::issue_key(std::path::Path::new(path))?;
            eprintln!("Owner key saved to the requested private file.");
            return Ok(());
        }
    }
    let desktop = match args.as_slice() {
        [] => None,
        [arg] if arg == "--desktop-stdio" => {
            Some(desktop_host::Bootstrap::read(&mut std::io::stdin())?)
        }
        _ => {
            return Err(
                "Usage: metrix-writer-api [--desktop-stdio | --init-owner-key <path>]".into(),
            )
        }
    };
    // Set native configuration before starting threads. Self-hosted mode retains
    // its explicit environment configuration and does not gain desktop privileges.
    if let Some(config) = &desktop {
        std::env::set_var("WRITER_DATA_DIR", &config.data_directory);
        std::env::set_var(
            "WRITER_VOCABULARY_PATH",
            config.data_directory.join("vocabulary.json"),
        );
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "metrix_writer_api=info,tower_http=info".into()),
        )
        .init();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(desktop))
}
async fn run(desktop: Option<desktop_host::Bootstrap>) -> Result<(), Box<dyn std::error::Error>> {
    let address: SocketAddr = if desktop.is_some() {
        "127.0.0.1:0".parse()?
    } else {
        let host = std::env::var("WRITER_API_HOST").unwrap_or_else(|_| "127.0.0.1".into());
        let port: u16 = std::env::var("WRITER_API_PORT")
            .unwrap_or_else(|_| "8791".into())
            .parse()?;
        format!("{host}:{port}").parse()?
    };
    let web_dist = desktop
        .as_ref()
        .map(|d| d.web_root.clone())
        .unwrap_or_else(|| {
            PathBuf::from(
                std::env::var("WRITER_WEB_DIST").unwrap_or_else(|_| "apps/web/dist".into()),
            )
        });
    // Validate the boundary before initializing storage or opening a listener.
    let boundary = if desktop.is_none() {
        Some(self_hosted::Boundary::from_env(address)?)
    } else {
        None
    };
    metrix_writer_api::jobs::runtime().await.map_err(|_| "Could not initialize private TrickTic Dictate storage; check data-directory permissions or another running instance.")?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    let bound = listener.local_addr()?;
    let router = app(ProviderConfiguration::from_env(), web_dist);
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    let router = if let Some(config) = desktop {
        // Only the inherited output pipe receives endpoint discovery; it is not health evidence.
        println!("{}", serde_json::json!({ "port": bound.port() }));
        std::io::stdout().flush()?;
        std::thread::spawn(move || {
            use std::io::Read;
            // Parent closes stdin on quit/crash. Any byte after bootstrap also requests stop.
            let _ = std::io::stdin().read(&mut [0]);
            let _ = shutdown.send(());
        });
        desktop_host::protect(router, config, bound)
    } else {
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = shutdown.send(());
        });
        tracing::info!(address = %bound, "TrickTic Dictate API listening");
        boundary.expect("self-hosted boundary").protect(router)
    };
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await?;
    Ok(())
}
