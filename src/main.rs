mod acp;
mod gateway;
mod msp;

use gateway::Gateway;

#[tokio::main]
async fn main() {
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "login") {
        std::process::exit(terminal_login().await);
    }
    if let Err(error) = run().await {
        eprintln!("[muse-acp] fatal: {error}");
        std::process::exit(1);
    }
}

/// Hidden terminal-auth entry point advertised by ACP as `args: ["login"]`.
async fn terminal_login() -> i32 {
    let binary = std::env::var("MUSE_CLI").unwrap_or_else(|_| "muse".to_string());
    match tokio::process::Command::new(&binary)
        .arg("login")
        .status()
        .await
    {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("[muse-acp] failed to run {binary} login: {error}");
            1
        }
    }
}

async fn run() -> Result<(), String> {
    let gateway = Gateway::new().await?;
    let result = acp::serve(gateway.clone()).await;
    gateway.host.shutdown().await;
    result.map_err(|error| error.to_string())
}
