#![feature(async_closure)]
#![feature(async_await)]

#[macro_use]
#[cfg(test)]
extern crate lazy_static;

pub use crate::request::BasicAuth;
use futures::future::join_all;
use futures::FutureExt;
use itertools::Itertools;
use serde::Deserialize;
use std::{future::Future, io};

mod request;

pub type Error = Box<dyn std::error::Error>;

#[derive(Deserialize)]
#[cfg_attr(test, derive(Debug, Clone, Eq, PartialEq))]
struct Repo {
    stargazers_count: usize,
    name: String,
    owner: RepoOwner,
}

#[derive(Deserialize)]
#[cfg_attr(test, derive(Debug, Clone, Eq, PartialEq))]
struct RepoOwner {
    login: String,
}

#[derive(Deserialize, Clone)]
struct User {
    login: String,
    public_repos: usize,
}

pub struct Options {
    pub auth: Option<BasicAuth>,
    pub page_size: usize,
    pub repo_limit: usize,
    pub stargazer_threshold: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            auth: None,
            page_size: 100,
            repo_limit: 10,
            stargazer_threshold: 0,
        }
    }
}

pub async fn count_stars(
    username: &str,
    out: impl io::Write,
    Options {
        auth,
        page_size,
        repo_limit,
        stargazer_threshold,
    }: Options,
) -> Result<(), Error> {
    use std::iter;

    let user_url = format!("users/{}", username);
    let user: User = request::json(user_url.clone(), auth.clone()).await?;
    let orgs_url = format!("{}/orgs", user_url);
    let orgs: Vec<RepoOwner> = request::json(orgs_url, auth.clone()).await?;

    // TODO make this into 'async' (without move) closure so we don't move these
    // It's strange that the move happening at the end is not allowed, it should be fine
    // to have the closure own these after they have been used.

    // TODO: is there an easy way to abort all unresolved futures if one of them failed?
    let orgs = futures::future::join_all(
        orgs.into_iter()
            .map(|user| request::json_log_failure::<User>(format!("users/{}",user.login), auth.clone())),
    )
    .await;
    let repos: Vec<_> = futures::future::join_all(
        iter::once(user)
            .chain(orgs.into_iter().filter_map(|v| v))
            .map(|user| {
                fetch_repos(user, page_size, |user, page_number| {
                    let repos_paged_url = format!(
                        "users/{}/repos?per_page={}&page={}",
                        user.login,
                        page_size,
                        page_number + 1
                    );
                    request::json_log_failure(repos_paged_url, auth.clone())
                        .map(|v| v.ok_or_else(|| "we will ignore errors".into()))
                })
            }),
    )
    .await
    .into_iter()
    .filter_map(|v| v.ok())
    .flatten()
    .collect();
    output(repos, repo_limit, stargazer_threshold, out)
}

async fn fetch_repos<F>(
    user: User, // TODO: can this also be &User?
    page_size: usize,
    mut fetch_page: impl FnMut(User, usize) -> F, // TODO would want 'async impl' for -> F; and &User instead of User!
) -> Result<Vec<Repo>, Error>
where
    F: Future<Output = Result<Vec<Repo>, Error>>,
{
    if page_size == 0 {
        return Err("PageSize must be greater than 0".into());
    }
    let page_count = user.public_repos / page_size;
    let futures = (0..=page_count).map(|page_number| fetch_page(user.clone(), page_number));
    let results: Vec<Result<Vec<Repo>, Error>> = join_all(futures).await;
    Ok(results
        .into_iter()
        .collect::<Result<Vec<_>, Error>>()?
        .into_iter()
        .concat())
}

fn output(
    mut repos: Vec<Repo>,
    repo_limit: usize,
    stargazer_threshold: usize,
    mut out: impl io::Write,
) -> Result<(), Error> {
    let total: usize = repos.iter().map(|r| r.stargazers_count).sum();

    writeln!(out, "Total: {}", total)?;

    repos.sort_by(|a, b| b.stargazers_count.cmp(&a.stargazers_count));
    let repos: Vec<_> = repos
        .iter()
        .filter(|r| r.stargazers_count >= stargazer_threshold)
        .take(repo_limit)
        .collect();
    let longest_name_len = repos.iter().map(|r| r.name.len()).max().unwrap_or(0);

    if repos.len() > 0 {
        writeln!(out)?;
    }
    for repo in repos {
        writeln!(
            out,
            "{:width$}   ★  {}",
            repo.name,
            repo.stargazers_count,
            width = longest_name_len
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
