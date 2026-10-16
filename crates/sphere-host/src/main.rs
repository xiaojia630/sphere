use anyhow::Result;
use clap::Parser;
use sphere_host::{serve, ServeOptions};
use std::path::PathBuf;

/// The Sphere process holder: owns agents and terminals across sphered restarts.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Socket to listen on.
    #[arg(long)]
    socket: PathBuf,
    /// Directory for the append-only output files.
    #[arg(long)]
    dir: PathBuf,
    /// Detach (setsid, double fork) and log to <dir>/host.log.
    #[arg(long)]
    daemonize: bool,
}

/// Classic double fork: the host ends up in its own session, parented by init,
/// so nothing that kills its starter can reach it.
fn daemonize(log: &std::path::Path) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    unsafe {
        match libc::fork() {
            -1 => return Err(std::io::Error::last_os_error()),
            0 => {}
            _ => libc::_exit(0),
        }
        if libc::setsid() < 0 {
            return Err(std::io::Error::last_os_error());
        }
        match libc::fork() {
            -1 => return Err(std::io::Error::last_os_error()),
            0 => {}
            _ => libc::_exit(0),
        }
    }
    let null = std::fs::File::open("/dev/null")?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
    unsafe {
        libc::dup2(null.as_raw_fd(), 0);
        libc::dup2(log.as_raw_fd(), 1);
        libc::dup2(log.as_raw_fd(), 2);
    }
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    if args.daemonize {
        std::fs::create_dir_all(&args.dir)?;
        daemonize(&args.dir.join("host.log"))?;
    }
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_ansi(false)
        .init();
    // The runtime is built after the fork: forking a running runtime is unsound.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(serve(ServeOptions {
            socket: args.socket,
            dir: args.dir,
            reap_orphans: true,
        }))
}
