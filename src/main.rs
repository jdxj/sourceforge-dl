mod sourceforge_downloader;

use clap::Parser;
use log::info;
use sourceforge_downloader::{SourceforgeDownloader, SourceforgeDownloaderConfig};
use tokio::join;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// sourceforge rss url
    rss_url: String,
    /// telegram user id
    user_id: u64,
    /// telegram bot token
    token: String,

    /// file save directory
    #[arg(long, default_value = "assets")]
    save_dir: String,
    /// static file url path
    #[arg(long, default_value = "/assets")]
    assets_path: String,
    /// static file server domain
    #[arg(long, default_value = "http://localhost:8080")]
    domain: String,
    /// crontab expression
    /// 秒 分 时 日 月 星期 年
    #[arg(long, default_value = "*/20 * * * * * *")]
    cron: String,
    /// static file server listen address
    #[arg(long, default_value = "0.0.0.0:8080")]
    listen_addr: String,
}

#[tokio::main]
async fn main() {
    env_logger::init();
    let cli = Cli::parse();

    let sdc = SourceforgeDownloaderConfig {
        rss_url: cli.rss_url,
        user_id: cli.user_id,
        token: cli.token,
        save_dir: cli.save_dir,
        assets_path: cli.assets_path,
        domain: cli.domain,
        cron: cli.cron,
        listen_addr: cli.listen_addr,
    };

    info!("starting");

    let sdl = SourceforgeDownloader::new(&sdc);
    join!(
        sdl.start_static_file_server(),
        sdl.start_get_latest_file_job()
    );

    info!("stopped");
}
