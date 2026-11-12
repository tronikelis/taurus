use std::{
    fs::{self, File},
    io::{BufRead, BufReader},
};

use anyhow::{Result as AnyResult, anyhow};
use curl::easy::{Easy, List as EasyList};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InfoResult {
    #[serde(rename = "ID")]
    pub id: i64,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "PackageBaseID")]
    pub package_base_id: i64,
    #[serde(rename = "PackageBase")]
    pub package_base: String,
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "Description")]
    pub description: Option<String>,
    #[serde(rename = "URL")]
    pub url: String,
    #[serde(rename = "NumVotes")]
    pub num_votes: i64,
    #[serde(rename = "Popularity")]
    pub popularity: f64,
    #[serde(rename = "OutOfDate")]
    pub out_of_date: Option<String>,
    #[serde(rename = "Maintainer")]
    pub maintainer: Option<String>,
    #[serde(rename = "FirstSubmitted")]
    pub first_submitted: i64,
    #[serde(rename = "LastModified")]
    pub last_modified: i64,
    #[serde(rename = "URLPath")]
    pub urlpath: Option<String>,
    #[serde(rename = "Depends")]
    pub depends: Option<Vec<String>>,
    #[serde(rename = "MakeDepends")]
    pub make_depends: Option<Vec<String>>,
    #[serde(rename = "License")]
    pub license: Option<Vec<String>>,
    #[serde(rename = "Keywords")]
    pub keywords: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Info {
    pub version: i64,
    pub r#type: String,
    #[serde(rename = "resultcount")]
    pub result_count: i64,
    pub results: Vec<InfoResult>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SearchResult {
    #[serde(rename = "ID")]
    pub id: i64,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Description")]
    pub description: Option<String>,
    #[serde(rename = "PackageBaseID")]
    pub package_base_id: i64,
    #[serde(rename = "PackageBase")]
    pub package_base: String,
    #[serde(rename = "Maintainer")]
    pub maintainer: Option<String>,
    #[serde(rename = "NumVotes")]
    pub num_votes: i64,
    #[serde(rename = "Popularity")]
    pub popularity: f64,
    #[serde(rename = "FirstSubmitted")]
    pub first_submitted: i64,
    #[serde(rename = "LastModified")]
    pub last_modified: i64,
    #[serde(rename = "OutOfDate")]
    pub out_of_date: Option<i64>,
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "URLPath")]
    pub url_path: Option<String>,
    #[serde(rename = "URL")]
    pub url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Search {
    pub version: i64,
    pub r#type: String,
    #[serde(rename = "resultcount")]
    pub result_count: i64,
    pub results: Vec<SearchResult>,
}

#[derive(Debug)]
pub struct Client {
    easy: Easy,
    base_url: String,
}

impl Client {
    pub fn new() -> Self {
        curl::init();
        Client {
            easy: Easy::new(),
            base_url: String::from("https://aur.archlinux.org"),
        }
    }

    fn init_easy(&mut self, url_path: &str) -> AnyResult<()> {
        self.easy.reset();
        let mut headers = EasyList::new();
        headers.append("Accept: application/json")?;
        headers.append(&format!("User-Agent: taurus/{}", env!("CARGO_PKG_VERSION")))?;
        self.easy.http_headers(headers)?;
        self.easy.url(&format!("{}{}", self.base_url, url_path))?;
        Ok(())
    }

    fn perform(&mut self) -> AnyResult<Vec<u8>> {
        let mut output = Vec::new();

        {
            let mut transfer = self.easy.transfer();
            transfer.write_function(|data| {
                if output.len() > 1 << 20 {
                    return Ok(0);
                }
                output.extend_from_slice(data);
                Ok(data.len())
            })?;
            transfer.perform()?;
        }

        let response_code = self.easy.response_code()?;
        if response_code != 200 {
            return Err(anyhow!("response code {} is not 200", response_code));
        }

        Ok(output)
    }

    pub fn info_package(&mut self, name: &str) -> AnyResult<InfoResult> {
        let info = self.info([name])?;
        let result = info
            .results
            .get(0)
            .ok_or_else(|| anyhow!("{} not found", &name))?;
        Ok(result.clone())
    }

    pub fn search(&mut self, package: &str) -> AnyResult<Search> {
        let url = format!(
            "/rpc/v5/search/{}",
            self.easy.url_encode(package.as_bytes())
        );
        self.init_easy(&url)?;
        let response: Search = serde_json::from_slice(&self.perform()?)?;

        if response.r#type == "error" {
            return Err(anyhow!("response type is error"));
        }

        Ok(response)
    }

    pub fn info<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) -> AnyResult<Info> {
        let mut url = format!("/rpc/v5/info?");
        for name in names {
            url.push_str(&format!("arg[]={}&", self.easy.url_encode(name.as_bytes())));
        }
        let url = &url[..url.len() - 1];

        self.init_easy(url)?;
        let response: Info = serde_json::from_slice(&self.perform()?)?;

        if response.r#type == "error" {
            return Err(anyhow!("response type is error"));
        }

        Ok(response)
    }
}

#[derive(Debug)]
struct PacmanDesc {
    name: String,
}

impl PacmanDesc {
    fn from_path(path: &str) -> AnyResult<Self> {
        let mut file = BufReader::new(File::open(path)?);

        let mut name: Option<String> = None;

        let mut buf = String::new();
        while {
            buf.clear();
            file.read_line(&mut buf)? != 0
        } {
            match &name {
                Some(v) => return Ok(Self { name: v.clone() }),
                _ => {}
            }

            match buf.trim() {
                "%NAME%" => {
                    buf.clear();
                    file.read_line(&mut buf)?;
                    name = Some(buf.trim().to_string());
                }
                _ => continue,
            }
        }

        Err(anyhow!("failed parsing pacman desc file"))
    }
}

pub fn is_package_installed(package: &str) -> AnyResult<bool> {
    for installed_package in fs::read_dir("/var/lib/pacman/local")? {
        let installed_package = installed_package?;

        let filename = installed_package
            .file_name()
            .into_string()
            .map_err(|v| anyhow!("cant convert {:?} to string", v))?;
        if !filename.starts_with(package) {
            continue;
        }

        let desc = PacmanDesc::from_path(
            installed_package
                .path()
                .join("desc")
                .to_str()
                .ok_or(anyhow!("path is not valid unicode"))?,
        )?;
        if desc.name == package {
            return Ok(true);
        }
    }

    Ok(false)
}
