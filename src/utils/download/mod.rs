use surf::{self, Error};

pub mod buffer;

pub const MAX_REDIRECT_COUNT: usize = 10;

pub async fn download_in_json<'a>(url: &'a str) -> Result<serde_json::Value, Error> {
    match surf::get(url).await {
        Ok(mut response) => {
            match response.body_json::<serde_json::Value>().await {
                Ok(data) => Ok(data),
                Err(e) => Err(e)
            }
        },
        Err(e) => {
            return Err(e)
        }
    }
}

pub async fn download(url: String) -> Result<Vec<u8>, String> {
    match surf::get(url).recv_bytes().await {
        Ok(response) => {
            return Ok(response)
        },
        Err(e) => {
            return Err(e.to_string())
        }
    }
}
