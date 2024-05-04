use axum::Router;
use chrono::{DateTime, Utc};
use delay_timer::prelude::*;
use futures_util::StreamExt;
use http::header::{ACCEPT, ACCEPT_ENCODING, RANGE};
use log::{debug, error};
use reqwest::header::HeaderMap;
use rss::Channel;
use std::{
    cmp::Ordering,
    error::Error,
    fmt::{Display, Formatter},
    fs::File,
    io::Write,
    path::Path,
    sync::Arc,
    time::Duration,
};
use teloxide::prelude::*;
use tower_http::services::ServeDir;

#[derive(Debug)]
struct FileMetaInfo {
    pub_date: DateTime<Utc>,
    download_url: String,
    md5: String,
    name: String,
    static_file_url: String,
}

impl FileMetaInfo {
    fn new(
        pub_date: &str,
        download_url: &str,
        md5: &str,
        name: &str,
        static_file_url: &str,
    ) -> Result<Self, Box<dyn Error>> {
        let date_time = DateTime::parse_from_rfc2822(pub_date)?.with_timezone(&Utc);

        Ok(FileMetaInfo {
            pub_date: date_time,
            download_url: download_url.to_string(),
            md5: md5.to_string(),
            name: name.to_string(),
            static_file_url: static_file_url.to_string(),
        })
    }
}

impl PartialEq for FileMetaInfo {
    fn eq(&self, other: &Self) -> bool {
        self.md5.len() > 0 && self.md5 == other.md5
    }
}

impl PartialOrd for FileMetaInfo {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self.pub_date.le(&other.pub_date) {
            Some(Ordering::Less)
        } else if self.pub_date.eq(&other.pub_date) {
            Some(Ordering::Equal)
        } else {
            Some(Ordering::Greater)
        }
    }
}

impl Display for FileMetaInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "\nfile name: {}\npub date: {}\ndownload url: {}\nmd5: {}\nstatic file url: {}",
            self.name, self.pub_date, self.download_url, self.md5, self.static_file_url,
        )
    }
}

pub struct SourceforgeDownloaderConfig {
    pub save_dir: String,
    pub assets_path: String,
    pub domain: String,
    pub cron: String,

    pub listen_addr: String,

    pub rss_url: String,
    pub user_id: u64,
    pub token: String,
}

pub struct SourceforgeDownloader {
    save_dir: String,
    assets_path: String,
    domain: String,
    cron: String,

    listen_addr: String,

    inner: Arc<SourceforgeDownloaderRef>,
    delay_timer: DelayTimer,
}

impl SourceforgeDownloader {
    pub fn new(config: &SourceforgeDownloaderConfig) -> Self {
        SourceforgeDownloader {
            save_dir: config.save_dir.to_string(),
            assets_path: config.assets_path.to_string(),
            domain: config.domain.to_string(),
            cron: config.cron.to_string(),
            listen_addr: config.listen_addr.to_string(),
            inner: Arc::new(SourceforgeDownloaderRef::new(
                &config.rss_url,
                config.user_id,
                &config.token,
            )),
            delay_timer: DelayTimer::new(),
        }
    }

    /// 启动静态文件服务
    pub async fn start_static_file_server(&self) {
        let app = Router::new().nest_service(&self.assets_path, ServeDir::new(&self.save_dir));

        let listener = tokio::net::TcpListener::bind(&self.listen_addr)
            .await
            .unwrap();
        axum::serve(listener, app).await.unwrap();
    }

    /// 定时获取最新文件
    pub async fn start_get_latest_file_job(&self) {
        // 避免 self 进入闭包导致 static 生命周期问题, 这里克隆一次
        let inner_clone = self.inner.clone();
        let save_dir_clone = self.save_dir.clone();
        let static_file_url_prefix = format!("{}{}", self.domain, self.assets_path);

        let get_latest_file_and_download = move || {
            // 再克隆一个 inner_clone 给 async 使用
            let inner_clone = inner_clone.clone();
            let save_dir_clone = save_dir_clone.clone();
            let static_file_url_prefix_clone = static_file_url_prefix.clone();

            async move {
                // 获取最新文件
                match inner_clone
                    .get_latest_file(&static_file_url_prefix_clone)
                    .await
                {
                    Ok(fmi) => {
                        let save_path = Path::new(&save_dir_clone).join(&fmi.name);
                        let static_file_url =
                            format!("{}/{}", static_file_url_prefix_clone, &fmi.name);
                        debug!(
                            "save_path: {:?}, static file url: {}",
                            save_path, static_file_url
                        );

                        // 下载过就不下载了
                        if let Ok(true) = save_path.try_exists() {
                            debug!("下载过: {:?}", save_path);
                            return;
                        }

                        // 启动一个新 task 来下载
                        tokio::spawn(async move {
                            if let Err(e) = inner_clone.download_file(&save_path, &fmi).await {
                                eprintln!("download file err: {:?}", e);
                            }
                        });
                    }
                    Err(e) => eprintln!("error: {:?}", e),
                }
            }
        };

        let task = TaskBuilder::default()
            .set_frequency_repeated_by_cron_str(&self.cron)
            // 把 inner_clone 移到 Fn 闭包中
            .spawn_async_routine(get_latest_file_and_download)
            .unwrap();
        self.delay_timer.add_task(task).unwrap()
    }
}

struct SourceforgeDownloaderRef {
    rss_url: String,
    http_client: reqwest::Client,

    chat_id: ChatId,
    tg_client: Bot,
}

impl SourceforgeDownloaderRef {
    fn new(rss_url: &str, user_id: u64, token: &str) -> Self {
        SourceforgeDownloaderRef {
            rss_url: rss_url.to_string(),
            http_client: new_http_client(),
            chat_id: UserId(user_id).into(),
            tg_client: Bot::new(token),
        }
    }

    /// 获取最新的文件信息
    async fn get_latest_file(
        &self,
        static_file_url_prefix: &str,
    ) -> Result<FileMetaInfo, Box<dyn Error>> {
        // 获取 rss 内容
        let req = self.http_client.get(&self.rss_url).build()?;
        let content = self.http_client.execute(req).await?.bytes().await?;

        // 解析 rss
        let channel = Channel::read_from(&content[..])?;
        // 获取最新的 rom 信息
        let latest_rom = channel.items.first().ok_or("latest rom not found")?;

        // 发布日期
        let pub_date = latest_rom.pub_date().ok_or("pub date not found")?;
        // 下载 url
        let download_url = latest_rom.link().ok_or("link not found")?;
        // md5
        let md5 = latest_rom
            .extensions()
            .get("media")
            .ok_or("media not found")?
            .get("content")
            .ok_or("content not found")?
            .first()
            .ok_or("content first extension not found")?
            .children()
            .get("hash")
            .ok_or("hash not found")?
            .first()
            .ok_or("hash first extension not found")?
            .value()
            .ok_or("md5 not found")?;
        // 文件名
        let name = Path::new(latest_rom.title().ok_or("title not found")?)
            .file_name()
            .ok_or("file name not found")?
            .to_str()
            .ok_or("file name can not to str")?;

        debug!("pub_date: {:?}, md5: {:?}, name: {:?}", pub_date, md5, name);

        let static_file_url = format!("{}/{}", static_file_url_prefix, name);
        let file = FileMetaInfo::new(pub_date, download_url, md5, name, &static_file_url)?;
        Ok(file)
    }

    /// 下载文件
    async fn download_file(
        &self,
        save_path: &Path,
        file_meta_info: &FileMetaInfo,
    ) -> Result<(), Box<dyn Error>> {
        // 重试限制
        let retry_limit = 5;
        let mut retry_num = 1;

        let mut file = File::create(save_path)?;
        let mut saved_content_len = 0u64;

        debug!("开始下载: {:?}", file_meta_info);
        'download_loop: loop {
            // 下载文件
            let req = self
                .http_client
                .get(&file_meta_info.download_url)
                .header(RANGE, format!("bytes={}-", saved_content_len))
                .build()?;
            let res = self.http_client.execute(req).await?;
            let mut stream = res.bytes_stream();
            // 保存到本地
            while let Some(item) = stream.next().await {
                match item {
                    Ok(chunk) => {
                        file.write_all(&chunk)?;
                        saved_content_len += chunk.len() as u64;
                    }
                    Err(e) => {
                        if retry_num >= retry_limit {
                            return Err(Box::new(e));
                        } else {
                            error!("下载出错: {:?}, 重试次数: {}", e, retry_limit);
                            retry_num += 1;
                            continue 'download_loop;
                        }
                    }
                }
            }
            break 'download_loop;
        }
        debug!(
            "下载完成: {:?}, 写入字节数: {}",
            file_meta_info, saved_content_len
        );
        file.flush()?;

        let text = format!("下载完成: {}", file_meta_info);
        self.send_message(&text).await;
        Ok(())
    }

    /// 发送 tg 消息
    async fn send_message(&self, text: &str) {
        if let Err(e) = self.tg_client.send_message(self.chat_id, text).await {
            error!("send message err: {:?}", e)
        }
    }
}

/// 创建 http 客户端
fn new_http_client() -> reqwest::Client {
    let mut header_map = HeaderMap::new();
    header_map.insert(ACCEPT, "*/*".parse().unwrap());
    header_map.insert(ACCEPT_ENCODING, "identity".parse().unwrap());

    reqwest::ClientBuilder::new()
        .connect_timeout(Duration::from_secs(10))
        .cookie_store(true)
        .user_agent("Wget/1.21.4")
        .default_headers(header_map)
        .build()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use crate::sourceforge_downloader::{
        SourceforgeDownloader, SourceforgeDownloaderConfig, SourceforgeDownloaderRef,
    };
    use std::path::Path;
    use std::{env, time::Duration};
    use tokio::time::sleep;

    fn setup() {
        env::set_var("RUST_LOG", "reqwest=trace,sourceforge_dl=debug");
        env_logger::init()
    }

    fn get_sourceforge_downloader_config() -> SourceforgeDownloaderConfig {
        let user_id = env::var("USER_ID").unwrap().parse::<u64>().unwrap();
        let token = env::var("TELOXIDE_TOKEN").unwrap();
        SourceforgeDownloaderConfig {
            save_dir: "assets".to_string(),
            assets_path: "/assets".to_string(),
            domain: "https://example.com".to_string(),
            cron: "*/20 * * * * * *".to_string(),
            listen_addr: "0.0.0.0:8080".to_string(),
            rss_url: "https://sourceforge.net/projects/bettercap.mirror/rss?path=/v2.32.0"
                .to_string(),
            user_id,
            token: token.to_string(),
        }
    }

    #[tokio::test]
    async fn get_latest_file() {
        setup();

        let sdl = SourceforgeDownloaderRef::new(
            "https://sourceforge.net/projects/evolution-x/rss?path=/raphael/14",
            123,
            "hello",
        );
        match sdl.get_latest_file("").await {
            Ok(file) => {
                println!("{:?}", file)
            }
            Err(e) => {
                eprintln!("{:?}", e)
            }
        }
    }

    #[tokio::test]
    async fn download_file() {
        setup();

        let user_id = env::var("USER_ID").unwrap().parse::<u64>().unwrap();
        let token = env::var("TELOXIDE_TOKEN").unwrap();

        let sdl = SourceforgeDownloaderRef::new(
            "https://sourceforge.net/projects/bettercap.mirror/rss?path=/v2.32.0",
            user_id,
            &token,
        );
        let file_meta_info = sdl.get_latest_file("").await.unwrap();
        let save_path = Path::new("/tmp").join(&file_meta_info.name);

        println!("download_url: {:?}", &file_meta_info.download_url);

        if let Err(e) = sdl.download_file(&save_path, &file_meta_info).await {
            eprintln!("{:?}", e)
        }
    }

    #[tokio::test]
    async fn test_send_message() {
        setup();

        let user_id = env::var("USER_ID").unwrap().parse::<u64>().unwrap();
        let token = env::var("TELOXIDE_TOKEN").unwrap();

        let sdl = SourceforgeDownloaderRef::new("", user_id, token.as_str());
        sdl.send_message("hello world").await
    }

    #[tokio::test]
    async fn file_server() {
        setup();

        let sdc = get_sourceforge_downloader_config();
        let sdl = SourceforgeDownloader::new(&sdc);

        sdl.start_static_file_server().await;
    }

    #[tokio::test]
    async fn star_get_latest_file_job() {
        setup();

        let sdc = get_sourceforge_downloader_config();
        let sdl = SourceforgeDownloader::new(&sdc);

        sdl.start_get_latest_file_job().await;
        sleep(Duration::from_secs(100)).await;
    }
}
