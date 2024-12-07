use reqwest::cookie;

static BASE_URL: LazyLock<Url> =
    LazyLock::new(|| Url::parse("https://api.cloudflare.com/client/v4").unwrap());

pub struct OmadaApi {
    config: OmadaConfig,
    base: Url,
    client: Client,
}

impl OmadaApi {
    pub fn new(config: OmadaConfig) -> Result<OmadaApi> {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "accept",
            header::HeaderValue::from_static("application/json"),
        );

        let builder = ClientBuilder::new()
            .add_root_certificate(Certificate::from_pem(include_bytes!("localhost.pem"))?)
            .use_rustls_tls()
            //This is due to omada generating a cert with a crap hostname
            .danger_accept_invalid_hostnames(true)
            .cookie_store(false);

        Ok(OmadaApi {
            config,
            base: Url::parse(&config.url)?,
            client: builder.build()?,
        })
    }

    pub async fn login(&mut self) -> anyhow::Result<LoginData> {
        let url = self.base.join("/api/v2/login")?;

        let mut post_body = HashMap::new();
        post_body.insert("username", self.config.username.clone());
        post_body.insert("password", self.config.password.clone());

        let response = self
            .client
            .post(url)
            .json(&post_body)
            .send()
            .await?
            .error_for_status()?;

        let session_cookie = response.cookies;

        let response_json = response.json::<LoginResult>().await?;

        Ok(SessionData {
            omadac_id: response_json.result.omadacId,
            token: response_json.result.token,
            session: session_cookie,
        })
    }

    pub async fn get_user_info(&self, session: SessionData) -> Result<UserInfo> {
        let user_info = self.base.join("/api/v2/current/users")?;
        let user_info_response = self
            .client
            .get(user_info.clone())
            .header("Csrf-Token", session.token.0)
            .send()
            .await?
            .error_for_status()?
            .json::<UserInfoResult>()
            .await?;

        Ok(user_info_response.result)

        //let site_id = user_info_json["result"]["privilege"]["sites"][0]
    }

    pub async fn get_controller_name(&self) -> Result<ControllerName> {
        let controller_status = self
            .base
            .join(&("/".to_string() + &omadac_id + "/api/v2/maintenance/controllerStatus"))?;
        let controller_status_response = self
            .client
            .get(controller_status.clone())
            .header("Csrf-Token", token.clone())
            .send()
            .await?
            .error_for_status()?;
        let controller_status_json = controller_status_response
            .json::<serde_json::Value>()
            .await?;

        let controller_reformatted = controller_status_json["result"]["macAddress"]
            .as_str()
            .ok_or_else(|| {
                anyhow!("Unable to find controller MAC").context(controller_status.clone())
            })?
            .replace(":", "-");
    }

    pub async fn get_wan_ip(&self) -> Result<Ipv4Addr> {
        let gateway_info = self.base.join(
            &("/".to_string()
                + &omadac_id
                + "/api/v2/sites/"
                + site_id
                + "/gateways/"
                + &controller_reformatted),
        )?;
        let gateway_info_response = self
            .client
            .get(gateway_info.clone())
            .header("Csrf-Token", token)
            .send()
            .await?
            .error_for_status()?;

        let gateway_info_json = gateway_info_response.json::<serde_json::Value>().await?;
        let port_info = gateway_info_json["result"]["portStats"]
            .as_array()
            .ok_or_else(|| anyhow!("Unable to find gateway ports").context(gateway_info.clone()))?;

        for port in port_info {
            let public_ip = port["wanPortIpv4Config"]["ip"].clone();
            if public_ip.is_string() {
                let parsed_ip = Ipv4Addr::from_str(public_ip.as_str().unwrap())?;
                return Ok(parsed_ip);
            }
        }

        Err(anyhow!("Unable to find wan ip").context(gateway_info.clone()))
    }
}

#[derive(Serialize, Deserialize)]
pub struct LoginResult {
    pub result: LoginData,
}

#[allow(non_snake_case)]
#[derive(Serialize, Deserialize)]
pub struct LoginData {
    pub omadacId: OmadacId,
    pub token: CSRFToken,
}

#[derive(Serialize, Deserialize)]
pub struct OmadacId(pub String);

#[derive(Serialize, Deserialize)]
pub struct CSRFToken(pub String);

#[derive(Serialize, Deserialize)]
pub struct SessionData {
    pub omadac_id: OmadacId,
    pub token: CSRFToken,
    pub session: Cookie,
}

#[derive(Serialize, Deserialize)]
pub struct UserInfoResult {
    pub result: UserInfo,
}

#[derive(Serialize, Deserialize)]
pub struct UserInfo {
    pub privilege: Privileges,
}

#[derive(Serialize, Deserialize)]
pub struct Privileges {
    pub sites: Vec<SiteId>,
}

#[derive(Serialize, Deserialize)]
pub struct SiteId(pub String);

#[derive(Serialize, Deserialize)]
pub struct ControllerName(pub String);
