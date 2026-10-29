"""Actual admitted-source semantic eligibility via the pinned maintained parser."""
from pathlib import Path
import tomllib

def validate_source(repo, sha):
    from git import Repo
    from semantic_release.commit_parser.conventional.options_monorepo import ConventionalCommitMonorepoParserOptions
    from semantic_release.commit_parser.conventional.parser_monorepo import ConventionalCommitMonorepoParser
    config = tomllib.loads(Path(__file__).with_name("python-semantic-release.toml").read_text(encoding="utf-8"))
    options = ConventionalCommitMonorepoParserOptions(**config["semantic_release"]["commit_parser_options"])
    parsed = ConventionalCommitMonorepoParser(options).parse_message(
        Repo(repo).commit(sha).message, strict_scope=True)
    if parsed is None or not parsed.scope or not parsed.scope.strip():
        raise ValueError("Actual merged source requires a supported scoped semantic subject before build/tag")
