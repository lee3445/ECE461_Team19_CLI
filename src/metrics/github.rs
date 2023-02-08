use crate::metrics::Metrics;
use chrono::{offset::Utc, Datelike};
use reqwest::header;
use statrs::distribution::{Continuous, ContinuousCDF, Normal};
use std::io::BufRead;

#[derive(Debug)]
pub struct Github {
    // repository information
    owner: String,
    repo: String,
    link: String,

    // API-related
    client: reqwest::blocking::Client,
}

impl Github {
    // create new instance with url
    pub fn with_url(url: &str) -> Option<Github> {
        let u = reqwest::Url::parse(url).ok()?;

        // check if domain is "github.com"
        if let Some(domain) = u.domain() {
            if domain != "github.com" {
                return None;
            }
        } else {
            return None;
        }

        // check if scheme is https or http
        let sch = u.scheme();
        if sch != "https" && sch != "http" {
            return None;
        }

        // extract repo info from url
        let mut path = u.path().split('/').skip(1);
        let link = url.to_string();
        let owner = path.next()?.to_string();
        let repo = path.next()?.to_string();

        // http client
        let mut headers = header::HeaderMap::new();
        let t = format!("Bearer {}", std::env::var("GITHUB_TOKEN").ok()?);
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(&t).ok()?,
        );
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "X-GitHub-Api-Version",
            header::HeaderValue::from_static("2022-11-28"),
        );
        let client = reqwest::blocking::Client::builder()
            .user_agent("ECE461_Team19_CLI")
            .default_headers(headers)
            .build()
            .ok()?;

        Some(Github {
            owner,
            repo,
            link,
            client,
        })
    }

    // GitHub REST API
    // https://docs.github.com/en/rest?apiVersion=2022-11-28
    pub fn rest_api(&self, path: &str) -> reqwest::Result<reqwest::blocking::Response> {
        self.client
            .get(format!(
                "https://api.github.com/repos/{}/{}/{}",
                self.owner, self.repo, path
            ))
            .send()
    }

    // REST API call with result in json format
    pub fn rest_json(&self, path: &str) -> reqwest::Result<serde_json::Value> {
        self.rest_api(path)?.json::<serde_json::Value>()
    }

    // GitHub GraphQL API
    pub fn graphql(&self, query: String) -> reqwest::Result<reqwest::blocking::Response> {
        self.client
            .post("https://api.github.com/graphql")
            .bearer_auth(format!("{}", std::env::var("GITHUB_TOKEN").unwrap()))
            .body(query)
            .send()
    }

    // GraphQL API call in json format
    pub fn graph_json(&self, query: String) -> reqwest::Result<serde_json::Value> {
        self.graphql(query)?.json::<serde_json::Value>()
    }

    // count how many pages the result has
    // see: https://docs.github.com/en/rest/guides/using-pagination-in-the-rest-api?apiVersion=2022-11-28
    pub fn rest_page_count(&self, path: &str) -> reqwest::Result<u32> {
        let response = self.rest_api(path)?;
        let header = response.headers().get("link");
        if header.is_none() {
            if response.json::<serde_json::Value>().unwrap()["message"].is_null() {
                return Ok(1);
            } else {
                return Ok(0);
            }
        }

        // get substring with the page number
        let res = header.unwrap().to_str().unwrap().split(',').nth(1).unwrap();
        // get page number
        let page = res.get(res.find("&page=").unwrap() + 6..res.find('>').unwrap());

        Ok(page.unwrap().parse::<u32>().unwrap())
    }
}
impl Metrics for Github {
    fn ramp_up_time(&self) -> f64 {
        // Specify the path of repo to clone into
        let path_name = format!("cloned_{}_{}", self.owner, self.repo);
        let repo_path = std::path::Path::new(&path_name);

        // Clone the repo
        git2::Repository::clone(&self.link, repo_path).unwrap();

        // Check if there is readme
        let file = match std::fs::File::open(&format!("{}/README.md", path_name)) {
            Ok(file) => file,
            Err(_) => {
                std::fs::remove_dir_all(repo_path).unwrap();
                return 0.0;
            }
        };
        let reader = std::io::BufReader::new(file);

        // Get the # of lines and calculate the score
        let lines = reader.lines().count();
        let result = Self::calc_ramp_up_time(lines.try_into().unwrap_or(u32::MAX));
        std::fs::remove_dir_all(repo_path).unwrap();
        result
    }

    fn correctness(&self) -> f64 {
        // issues returns pull requests as well, so subtract pulls from issues
        let all = self.rest_page_count("issues?state=all&per_page=1").unwrap()
            - self.rest_page_count("pulls?state=all&per_page=1").unwrap();
        let closed = self
            .rest_page_count("issues?state=closed&per_page=1")
            .unwrap()
            - self
                .rest_page_count("pulls?state=closed&per_page=1")
                .unwrap();
        Self::calc_correctness(all, closed)
    }

    fn bus_factor(&self) -> f64 {
        // call graphql api to get the data specified in the query
        let bus = self.graph_json(
            format!("{{\"query\" : \"query {{ repository(owner:\\\"{}\\\", name:\\\"{}\\\") {{ mentionableUsers {{ totalCount }} }} }}\" }}", self.owner, self.repo)
            ).unwrap();
        let collaborators = bus["data"]["repository"]["mentionableUsers"]["totalCount"]
            .as_i64()
            .unwrap();
        // calculate the score for bus factor
        let score: f64 = ((2.0 * collaborators as f64) / (collaborators as f64 + 1.0)) - 1.0;
        return score;
    }

    fn responsiveness(&self) -> f64 {
        // get pull requests last year with GraphQL API
        // source of query:
        // https://stackoverflow.com/questions/61477294/how-to-filter-github-pull-request-by-updated-date-using-graphql
        let a_year_ago = (Utc::now() - chrono::naive::Days::new(365)).format("%Y-%m-%d");
        let json = self.graph_json(
            format!("{{\"query\" : \"query {{ search(query: \\\"repo:{}/{} is:pr updated:>={}\\\" type:ISSUE) {{ issueCount }} }}\" }}", self.owner, self.repo, a_year_ago)
            ).unwrap();
        let pulls = json["data"]["search"]["issueCount"].as_f64().unwrap();

        let normal = Normal::new(0.0, 1.0).unwrap();

        normal.cdf(pulls / 13.0 - 2.0)
    }

    fn compatibility(&self) -> f64 {
        // get license with github api
        let l = self.rest_json("license").unwrap();
        let license = l["license"]["spdx_id"].as_str();

        // no license found
        if license.is_none() {
            return 0.0;
        }

        Self::calc_compatibility(&license.unwrap())
    }
}

#[cfg(test)] // needs $GITHUB_TOKEN
mod tests {
    use super::*;

    // testing with_url()
    #[test]
    fn construct_with_url() {
        let a = Github::with_url("https://github.com/lee3445/ECE461_Team19_CLI").unwrap();

        assert_eq!(a.owner, "lee3445");
        assert_eq!(a.repo, "ECE461_Team19_CLI");
    }

    #[test]
    fn construct_with_bad_url() {
        // not an url
        assert!(Github::with_url("not an url").is_none());

        // not a github url
        assert!(Github::with_url("127.0.0.1").is_none());
        assert!(Github::with_url(
            "https://doc.rust-lang.org/rust-by-example/testing/unit_testing.html"
        )
        .is_none());

        // not a repo url
        assert!(Github::with_url("https://github.com").is_none());
        assert!(Github::with_url("https://github.com/rust-lang").is_none());
    }

    // testing rest_json()
    #[test]
    fn rest_api_stargazers() -> reqwest::Result<()> {
        let g = Github::with_url("https://github.com/seanmonstar/reqwest").unwrap();
        assert_eq!(
            30,
            g.rest_json("stargazers").unwrap().as_array().unwrap().len()
        );
        Ok(())
    }

    // testing ramp_up_time
    #[test]
    fn ramp_up_time_no_readme() {
        let g = Github::with_url("https://github.com/phil-opp/llvm-tools").unwrap();
        assert_eq!(0.0, g.ramp_up_time());
    }

    #[test]
    fn ramp_up_time_normal_case() {
        let g = Github::with_url("https://github.com/ppy/osu").unwrap();
        assert!(g.ramp_up_time() > 0.0);
    }

    #[test]
    fn ramp_up_time_max() {
        // 147 lines
        let g = Github::with_url("https://github.com/graphql/graphql-js").unwrap();
        assert!(g.ramp_up_time() >= 0.99);
    }

    // testing correctness
    #[test]
    fn correctness_no_issues() {
        let g = Github::with_url("https://github.com/thinkloop/map-or-similar").unwrap();
        assert!(g.correctness() == 0.0);
    }

    #[test]
    fn correctness_max() {
        // 0 open, 1 closed issues
        let g = Github::with_url("https://github.com/crypto-browserify/md5.js").unwrap();
        assert!(g.correctness() == 1.0);
    }

    #[test]
    fn correctness_normal_case() {
        let g = Github::with_url("https://github.com/neovim/neovim").unwrap();
        assert!(g.correctness() >= 0.0);
    }

    // testing compatibility
    #[test]
    fn compatibility_no_license() {
        let g = Github::with_url("https://github.com/cloudinary/cloudinary_npm").unwrap();
        assert!(g.compatibility() == 0.0);
    }

    #[test]
    fn compatibility_lgpl_3() {
        let g = Github::with_url("https://github.com/haskell/ghcup-hs").unwrap();
        assert!(g.compatibility() == 1.0);
    }

    #[test]
    fn compatibility_mit() {
        let g = Github::with_url("https://github.com/microsoft/vscode").unwrap();
        assert!(g.compatibility() == 1.0);
    }

    #[test]
    fn compatibility_apache() {
        let g = Github::with_url("https://github.com/haskell/haskell-language-server").unwrap();
        assert!(g.compatibility() == 0.0);
    }
}
