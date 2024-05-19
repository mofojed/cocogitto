use cocogitto_changelog::release::{ChangelogCommit, Release};
use cocogitto_changelog::template::{RemoteContext, Template};

use crate::CocoGitto;
use anyhow::anyhow;
use anyhow::Result;
use chrono::Utc;
use cocogitto_changelog::error::ChangelogError;
use cocogitto_commit::Commit;
use cocogitto_config::SETTINGS;
use cocogitto_git::rev::CommitIter;
use cocogitto_oid::OidOf;
use colored::Colorize;
use log::warn;

impl CocoGitto {
    /// ## Get a changelog between two oids
    /// - `from` default value:latest tag or else first commit
    /// - `to` default value:`HEAD` or else first commit
    pub fn get_changelog(&self, pattern: &str, _with_child_releases: bool) -> Result<Release> {
        let commit_range = self.repository.revwalk(pattern)?;
        release_from_commits(commit_range).map_err(Into::into)
    }

    pub fn get_changelog_at_tag(&self, tag: &str, template: Template) -> Result<String> {
        let changelog = self.get_changelog(tag, false)?;

        changelog
            .into_markdown(template)
            .map_err(|err| anyhow!(err))
    }
}

pub fn get_template_context() -> Option<RemoteContext> {
    let remote = SETTINGS.changelog.remote.as_ref().cloned();

    let repository = SETTINGS.changelog.repository.as_ref().cloned();

    let owner = SETTINGS.changelog.owner.as_ref().cloned();

    RemoteContext::try_new(remote, repository, owner)
}

pub fn get_changelog_template() -> std::result::Result<Template, ChangelogError> {
    let context = get_template_context();
    let template = SETTINGS.changelog.template.as_deref().unwrap_or("default");

    Template::from_arg(template, context)
}

pub fn get_package_changelog_template() -> std::result::Result<Template, ChangelogError> {
    let context = get_template_context();
    let template = SETTINGS
        .changelog
        .package_template
        .as_deref()
        .unwrap_or("package_default");

    let template = match template {
        "remote" => "package_remote",
        "full_hash" => "package_full_hash",
        template => template,
    };

    Template::from_arg(template, context)
}

pub fn get_monorepo_changelog_template() -> std::result::Result<Template, ChangelogError> {
    let context = get_template_context();
    let template = SETTINGS
        .changelog
        .template
        .as_deref()
        .unwrap_or("monorepo_default");

    let template = match template {
        "remote" => "monorepo_remote",
        "full_hash" => "monorepo_full_hash",
        template => template,
    };

    Template::from_arg(template, context)
}

pub fn release_from_commits(
    commits: CommitIter<'_>,
) -> std::result::Result<Release, ChangelogError> {
    let mut releases = vec![];
    let mut commit_iter = commits.into_iter().rev().peekable();

    while let Some((_oid, _commit)) = commit_iter.peek() {
        let mut release_commits = vec![];

        for (oid, commit) in commit_iter.by_ref() {
            if matches!(oid, OidOf::Tag(_)) {
                release_commits.push((oid, commit));
                break;
            }
            release_commits.push((oid, commit));
        }

        release_commits.reverse();
        releases.push(release_commits);
    }

    let mut current = None;

    for release in releases {
        let next = Release {
            version: release.first().unwrap().0.clone(),
            from: current
                .as_ref()
                .map(|current: &Release| current.version.clone())
                .unwrap_or(release.last().unwrap().0.clone()),
            date: Utc::now().naive_local(),
            commits: release
                .iter()
                .filter_map(|(_, commit)| {
                    match Commit::from_git_commit(commit, &SETTINGS.allowed_commit_types()) {
                        Ok(commit) => {
                            let commit_type = &commit.conventional.commit_type;
                            if !SETTINGS.should_omit_commit(commit_type) {
                                let author_username =
                                    cocogitto_config::commit_username(&commit.author);
                                let changelog_title = SETTINGS.get_changelog_title(commit_type);
                                Some(ChangelogCommit::from_commit(
                                    commit,
                                    author_username,
                                    changelog_title,
                                ))
                            } else {
                                None
                            }
                        }
                        Err(err) => {
                            let err = err.to_string().red();
                            warn!("{}", err);
                            None
                        }
                    }
                })
                .collect(),
            previous: current.map(Box::new),
        };

        current = Some(next);
    }

    current.ok_or(ChangelogError::EmptyRelease)
}

#[cfg(test)]
mod test {
    use crate::command::changelog::release_from_commits;

    use cocogitto_git::tag::TagLookUpOptions;
    use cocogitto_git::Repository;
    use cocogitto_oid::OidOf;
    use cocogitto_test_helpers::open_cocogitto_repo;
    use cocogitto_test_helpers::*;
    use git2::Oid;
    use sealed_test::prelude::sealed_test;
    use sealed_test::prelude::*;
    use speculoos::prelude::*;

    #[test]
    fn should_get_a_release() -> anyhow::Result<()> {
        let repo = open_cocogitto_repo()?;
        let iter = repo.revwalk("..")?;
        let release = release_from_commits(iter);
        assert_that!(release)
            .is_ok()
            .matches(|r| !r.commits.is_empty());
        Ok(())
    }

    #[sealed_test]
    fn shoud_get_range_for_a_single_release() -> anyhow::Result<()> {
        // Arrange
        let repo = git_init_no_gpg()?;
        let one = commit("chore: first commit")?;
        let two = commit("feat: feature 1")?;
        let three = commit("feat: feature 2")?;
        git_tag("0.1.0")?;

        let range = repo.revwalk("0.1.0");

        let range = range?;

        // Act
        let release = release_from_commits(range)?;

        // Assert
        assert_that!(release.previous).is_none();
        assert_that!(release.version.oid()).is_equal_to(&Oid::from_str(&three)?);
        assert_that!(release.from).is_equal_to(OidOf::FirstCommit(Oid::from_str(&one)?));

        let expected_commits: Vec<String> = release
            .commits
            .into_iter()
            .map(|commit| commit.commit.oid)
            .collect();

        assert_that!(expected_commits).is_equal_to(vec![three, two, one]);

        Ok(())
    }

    #[sealed_test]
    fn shoud_get_range_for_a_multiple_release() -> anyhow::Result<()> {
        // Arrange
        let repo = git_init_no_gpg()?;
        let one = commit("chore: first commit")?;
        let two = commit("feat: feature 1")?;
        let three = commit("feat: feature 2")?;
        git_tag("0.1.0")?;
        let four = commit("feat: feature 3")?;
        let five = commit("feat: feature 4")?;
        git_tag("0.2.0")?;

        let range = repo.revwalk("..0.2.0")?;

        // Act
        let release = release_from_commits(range)?;

        // Assert
        assert_that!(release.previous).is_some().matches(|_child| {
            let commits: Vec<String> = release
                .previous
                .as_ref()
                .unwrap()
                .commits
                .iter()
                .map(|commit| commit.commit.oid.clone())
                .collect();

            commits == [three.clone(), two.clone(), one.clone()]
        });

        assert_that!(release.version.to_string()).is_equal_to("0.2.0".to_string());
        assert_that!(release.from.to_string()).is_equal_to("0.1.0".to_string());

        let expected_commits: Vec<String> = release
            .commits
            .into_iter()
            .map(|commit| commit.commit.oid)
            .collect();

        assert_that!(expected_commits).is_equal_to(vec![five, four]);

        Ok(())
    }

    #[test]
    fn get_release_range_integration_test() -> anyhow::Result<()> {
        // Arrange
        let repo = open_cocogitto_repo()?;
        let range = repo.revwalk("0.32.1..0.32.3")?;

        // Act
        let release = release_from_commits(range)?;

        // Assert
        assert_that!(release.version.to_string()).is_equal_to("0.32.3".to_string());

        let release = *release.previous.unwrap();
        assert_that!(release.version.to_string()).is_equal_to("0.32.2".to_string());

        assert_that!(release.previous).is_none();
        Ok(())
    }

    #[test]
    fn recursive_from_origin_to_head() -> anyhow::Result<()> {
        // Arrange
        let repo = Repository::open(&get_workspace_root())?;
        let mut tag_count = repo.tag_names(None)?.len();
        let head = repo.get_head_commit_oid()?;
        let latest = repo.get_latest_tag(TagLookUpOptions::default())?;
        let latest = latest.oid();
        if latest == Some(&head) {
            tag_count -= 1;
        };

        let range = repo.revwalk("..")?;

        // Act
        let mut release = release_from_commits(range)?;
        let mut count = 0;

        while let Some(previous) = release.previous {
            release = *previous;
            count += 1;
        }

        // Assert
        assert_that!(count).is_equal_to(tag_count);

        Ok(())
    }

    #[sealed_test]
    fn from_commit_to_head() -> anyhow::Result<()> {
        // Arrange
        let repo = git_init_no_gpg()?;

        commit("chore: init")?;
        commit("feat: a commit")?;
        let one = commit("chore: another commit")?;
        let two = commit("feat: a feature")?;
        let three = commit("chore: 1.0.0")?;
        let four = commit("fix: the bug")?;

        let range = repo.revwalk(&format!("{}..", &one[0..7]))?;

        // Act
        let release = release_from_commits(range)?;

        // Assert
        let actual_oids: Vec<String> = release
            .commits
            .iter()
            .map(|commit| commit.commit.oid.to_string())
            .collect();

        assert_that!(actual_oids).is_equal_to(vec![four, three, two]);

        Ok(())
    }

    #[sealed_test]
    fn from_commit_to_head_with_overlapping_tag() -> anyhow::Result<()> {
        // Arrange
        let repo = git_init_no_gpg()?;

        commit("chore: init")?;
        commit("feat: a commit")?;

        let from = commit("chore: another commit")?;
        let one = commit("feat: a feature")?;
        let two = commit("chore: 1.0.0")?;
        git_tag("1.0.0")?;
        let three = commit("fix: the bug")?;

        let range = repo.revwalk(&format!("{}..", &from[0..7]))?;

        // Act
        let release = release_from_commits(range)?;

        // Assert
        let head_to_v1: Vec<String> = release
            .commits
            .iter()
            .map(|commit| commit.commit.oid.to_string())
            .collect();

        let commit_before_v1: Vec<String> = release
            .previous
            .unwrap()
            .commits
            .iter()
            .map(|commit| commit.commit.oid.to_string())
            .collect();

        assert_that!(head_to_v1).is_equal_to(vec![three]);
        assert_that!(commit_before_v1).is_equal_to(vec![two, one]);

        Ok(())
    }
}
