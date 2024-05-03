FROM debian:12-slim

RUN apt-get update && apt-get install -y libssl-dev ca-certificates openssl && rm -rf /var/lib/apt/lists/*

RUN mkdir -p /sourceforge-downloader
COPY sourceforge-dl /sourceforge-downloader/sourceforge-dl
