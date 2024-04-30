use std::time::Duration;
use std::error::Error;
use rss::Channel;
use chrono::{DateTime, Utc};

#[derive(Debug)]
struct File {
    pub_date: DateTime<Utc>,
    download_url: String,
    md5: String,
}

impl File {
    fn new(pub_date: &str, download_url: &str, md5: &str) -> Result<Self, Box<dyn Error>> {
        let date_time = DateTime::parse_from_rfc2822(pub_date)?
            .with_timezone(&Utc);

        Ok(File {
            pub_date: date_time,
            download_url: download_url.to_string(),
            md5: md5.to_string(),
        })
    }
}

pub struct SourceforgeDownloader {
    rss_url: String,
    http_client: reqwest::Client,

}

impl SourceforgeDownloader {
    pub fn new(rss_url: &str) -> Self {
        SourceforgeDownloader {
            rss_url: rss_url.to_string(),
            http_client: new_http_client(),
        }
    }

    async fn get_latest_file(&self) -> Result<File, Box<dyn Error>> {
        // 获取 rss 内容
        let req = self.http_client.get(&self.rss_url).build()?;
        let content = self.http_client.execute(req)
            .await?
            .bytes()
            .await?;

        // 解析 rss
        let channel = Channel::read_from(&content[..])?;
        // 获取最新的 rom 信息
        let latest_rom = channel.items.first().ok_or("latest rom not found")?;

        // 发布日期
        let pub_date = latest_rom.pub_date().ok_or("pub date not found")?;
        // 下载 url
        let download_url = latest_rom.link().ok_or("link not found")?;
        // md5
        let md5 = latest_rom.extensions()
            .get("media").ok_or("media not found")?
            .get("content").ok_or("content not found")?
            .first().ok_or("content first extension not found")?
            .children()
            .get("hash").ok_or("hash not found")?
            .first().ok_or("hash first extension not found")?
            .value().ok_or("md5 not found")?;

        println!("pub_date: {:?}, md5: {:?}", pub_date, md5);

        let file = File::new(pub_date, download_url, md5)?;
        Ok(file)
    }
}

fn new_http_client() -> reqwest::Client {
    reqwest::ClientBuilder::new()
        .connect_timeout(Duration::from_secs(10))
        .cookie_store(true)
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36")
        .build()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use crate::sourceforge_downloader::SourceforgeDownloader;

    #[tokio::test]
    async fn get_latest_file() {
        let sdl = SourceforgeDownloader::new("https://sourceforge.net/projects/evolution-x/rss?path=/raphael/14");
        match sdl.get_latest_file().await {
            Ok(file) => {
                println!("{:?}", file)
            }
            Err(e) => {
                eprintln!("{:?}", e)
            }
        }
    }
}
