use reqwest::Client;
use serde::{Deserialize, Serialize};
use anyhow::Result;

/// B 站登录客户端
pub struct BilibiliLogin {
    client: Client,
}

impl BilibiliLogin {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// 获取 QR 码
    pub async fn get_qrcode(&self) -> Result<QrcodeResult> {
        let url = "https://passport.bilibili.com/x/passport-login/web/qrcode/generate";
        let resp = self.client.get(url).send().await?;
        let json: serde_json::Value = resp.json().await?;

        if json["code"] != 0 {
            anyhow::bail!("获取 QR 码失败: {}", json["message"]);
        }

        let data = &json["data"];
        let qrcode_url = data["url"].as_str().unwrap();
        let qrcode_key = data["qrcode_key"].as_str().unwrap();

        // 生成 QR 码图片 (使用 qrcode server)
        let qrcode_image = format!("https://api.qrserver.com/v1/create-qr-code/?size=200x200&data={}",
            urlencoding::encode(qrcode_url));

        Ok(QrcodeResult {
            url: qrcode_url.to_string(),
            qrcode_key: qrcode_key.to_string(),
            qrcode_image,
        })
    }

    /// 轮询登录状态
    pub async fn poll_login_status(&self, qrcode_key: &str) -> LoginStatus {
        let url = format!(
            "https://passport.bilibili.com/x/passport-login/web/qrcode/poll?qrcode_key={}",
            qrcode_key
        );

        match self.client.get(&url).send().await {
            Ok(resp) => {
                match resp.json::<serde_json::Value>().await {
                    Ok(json) => {
                        let code = json["data"]["code"].as_i64().unwrap_or(-1);
                        match code {
                            // 0: 登录成功
                            0 => LoginStatus::Success {
                                url: json["data"]["url"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string(),
                                refresh_token: json["data"]["refresh_token"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_string(),
                            },
                            86038 => LoginStatus::Expired,
                            // 86101: 未扫码, 86090: 已扫码未确认
                            86101 | 86090 => LoginStatus::Waiting,
                            _ => LoginStatus::Failed,
                        }
                    }
                    Err(_) => LoginStatus::Failed,
                }
            }
            Err(_) => LoginStatus::Failed,
        }
    }
}

impl Default for BilibiliLogin {
    fn default() -> Self {
        Self::new()
    }
}

/// QR 码结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QrcodeResult {
    pub url: String,
    pub qrcode_key: String,
    pub qrcode_image: String,
}

/// 登录状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LoginStatus {
    Waiting,      // 等待扫码
    Expired,      // QR 码过期
    Success { url: String, refresh_token: String },  // 登录成功
    Failed,       // 失败
}
