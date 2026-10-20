/// GitHub の origin から `owner/repo` だけを取り出す。
///
/// host は `github.com` と完全一致するときだけ受け付ける。userinfo の token は結果に残さない。
pub fn github_repo(url: &str) -> Option<String> {
    let path = repo_path(url.trim())?;
    let repo = path.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = repo.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(name), None)
            if !owner.is_empty()
                && !name.is_empty()
                && !owner.contains('@')
                && !name.contains('@') =>
        {
            Some(format!("{owner}/{name}"))
        }
        _ => None,
    }
}

fn repo_path(url: &str) -> Option<&str> {
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        return Some(rest);
    }
    if let Some(rest) = url.strip_prefix("ssh://") {
        let (host, path) = split_authority(rest)?;
        return (host == "github.com").then_some(path);
    }
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = split_authority(rest)?;
    (host == "github.com").then_some(path)
}

fn split_authority(rest: &str) -> Option<(&str, &str)> {
    let (authority, path) = rest.split_once('/')?;
    let host = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let host = host.split_once(':').map(|(host, _)| host).unwrap_or(host);
    Some((host, path))
}

#[cfg(test)]
mod tests {
    use super::github_repo;

    #[test]
    fn github_repo_accepts_owner_and_repo_only() {
        assert_eq!(
            github_repo("git@github.com:owner/repo.git"),
            Some("owner/repo".into())
        );
        assert_eq!(
            github_repo("https://github.com/owner/repo"),
            Some("owner/repo".into())
        );
        assert_eq!(
            github_repo("ssh://git@github.com/owner/repo.git"),
            Some("owner/repo".into())
        );
        assert_eq!(github_repo("https://github.com/owner/repo/extra"), None);
        assert_eq!(github_repo("https://other/owner/repo"), None);
        assert_eq!(github_repo("https://notgithub.com/owner/repo"), None);
        let token =
            github_repo("https://x-access-token:secret@github.com/o/n/").expect("token url");
        assert_eq!(token, "o/n");
        assert!(!token.contains("secret"));
        assert!(!token.contains("x-access-token"));
    }
}
