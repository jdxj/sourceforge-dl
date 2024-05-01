use axum::Router;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use http::header::{ACCEPT, ACCEPT_ENCODING};
use log::{debug, error, info};
use reqwest::header::HeaderMap;
use rss::Channel;
use std::{cmp::Ordering, error::Error, fs::File, io::Write, path::Path, time::Duration};
use teloxide::prelude::*;
use tower_http::services::ServeDir;

#[derive(Debug)]
struct FileMetaInfo {
    pub_date: DateTime<Utc>,
    download_url: String,
    md5: String,
    name: String,
}

impl FileMetaInfo {
    fn new(
        pub_date: &str,
        download_url: &str,
        md5: &str,
        name: &str,
    ) -> Result<Self, Box<dyn Error>> {
        let date_time = DateTime::parse_from_rfc2822(pub_date)?.with_timezone(&Utc);

        Ok(FileMetaInfo {
            pub_date: date_time,
            download_url: download_url.to_string(),
            md5: md5.to_string(),
            name: name.to_string(),
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

pub struct SourceforgeDownloader {
    rss_url: String,
    http_client: reqwest::Client,

    chat_id: ChatId,
    tg_client: Bot,
}

impl SourceforgeDownloader {
    pub fn new(rss_url: &str, user_id: u64, token: &str) -> Self {
        SourceforgeDownloader {
            rss_url: rss_url.to_string(),
            http_client: new_http_client(),
            chat_id: UserId(user_id).into(),
            tg_client: Bot::new(token),
        }
    }

    /// 获取最新的文件信息
    async fn get_latest_file(&self) -> Result<FileMetaInfo, Box<dyn Error>> {
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

        let file = FileMetaInfo::new(pub_date, download_url, md5, name)?;
        Ok(file)
    }

    /// 下载文件
    async fn download_file(
        &self,
        save_path: &str,
        download_url: &str,
    ) -> Result<(), Box<dyn Error>> {
        // 下载文件
        let req = self.http_client.get(download_url).build()?;
        let res = self.http_client.execute(req).await?;
        let mut stream = res.bytes_stream();
        // 保存到本地
        let mut file = File::create(save_path)?;
        while let Some(item) = stream.next().await {
            let chunk = item?;
            file.write_all(&chunk)?;
        }
        Ok(())
    }

    /// 发送 tg 消息
    async fn send_message(&self, text: &str) {
        if let Err(e) = self.tg_client.send_message(self.chat_id, text).await {
            error!("send message err: {:?}", e)
        }
    }

    /// 启动静态文件服务
    async fn start_static_file_server(&self) {
        let app = Router::new().nest_service("/assets", ServeDir::new("assets"));

        let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
        axum::serve(listener, app).await.unwrap();
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
    use crate::sourceforge_downloader::SourceforgeDownloader;
    use std::env;

    fn setup() {
        env::set_var("RUST_LOG", "reqwest=trace,sourceforge_dl=debug");
        env_logger::init()
    }

    #[tokio::test]
    async fn get_latest_file() {
        setup();

        let sdl = SourceforgeDownloader::new(
            "https://sourceforge.net/projects/evolution-x/rss?path=/raphael/14",
            123,
            "hello",
        );
        match sdl.get_latest_file().await {
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

        let sdl = SourceforgeDownloader::new(
            "https://sourceforge.net/projects/bettercap.mirror/rss?path=/v2.32.0",
            123,
            "hello",
        );
        let file_meta_info = sdl.get_latest_file().await.unwrap();
        let save_path = "/tmp/".to_string() + &file_meta_info.name;

        println!("download_url: {:?}", &file_meta_info.download_url);

        if let Err(e) = sdl
            .download_file(&save_path, &file_meta_info.download_url)
            .await
        {
            eprintln!("{:?}", e)
        }
    }

    #[tokio::test]
    async fn test_send_message() {
        setup();

        let user_id = env::var("USER_ID").unwrap().parse::<u64>().unwrap();
        let token = env::var("TELOXIDE_TOKEN").unwrap();

        let sdl = SourceforgeDownloader::new("", user_id, token.as_str());
        sdl.send_message("hello world").await
    }

    #[tokio::test]
    async fn file_server() {
        setup();

        let sdl = SourceforgeDownloader::new("", 0, "");

        sdl.start_static_file_server().await;
    }
}
