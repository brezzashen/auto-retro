//! A blocking GET for the CLI commands (`fetch` in the TypeScript): follows redirects, no
//! decompression requested, an optional overall timeout.

use std::time::Duration;

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// `response.ok`: a 2xx status.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.clone())
    }

    pub fn text(&self) -> String {
        let text = String::from_utf8_lossy(&self.body);
        text.strip_prefix('\u{FEFF}').unwrap_or(&text).to_string()
    }
}

pub fn get(url: &str, headers: &[(&str, String)], timeout: Option<Duration>) -> Result<Response, String> {
    send(reqwest::Method::GET, url, headers, None, timeout)
}

/// A blocking request with an optional body (`POST`, `PATCH`).
pub fn send(method: reqwest::Method, url: &str, headers: &[(&str, String)], body: Option<Vec<u8>>, timeout: Option<Duration>) -> Result<Response, String> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let mut builder = reqwest::Client::builder();
        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }
        let client = builder.build().map_err(|e| e.to_string())?;
        let mut request = client.request(method, url);
        if let Some(body) = body {
            request = request.body(body);
        }
        for (key, value) in headers {
            request = request.header(*key, value);
        }
        let response = request.send().await.map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let headers = response.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string())).collect();
        let body = response.bytes().await.map_err(|e| e.to_string())?.to_vec();
        Ok(Response { status, headers, body })
    })
}
