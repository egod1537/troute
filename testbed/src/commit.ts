const GITHUB_REPOSITORY_URL = "https://github.com/egod1537/troute";
const GIT_SHA_PATTERN = /^[0-9a-f]{7,40}$/i;

export interface CommitMetadata {
  fullSha: string | null;
  shortSha: string;
  url: string | null;
}

export function resolveCommitMetadata(value: string | undefined): CommitMetadata {
  const sha = value?.trim() ?? "";
  if (!GIT_SHA_PATTERN.test(sha)) {
    return { fullSha: null, shortSha: "unknown", url: null };
  }

  return {
    fullSha: sha,
    shortSha: sha.slice(0, 7),
    url: `${GITHUB_REPOSITORY_URL}/commit/${sha}`,
  };
}

export const BUILD_COMMIT = resolveCommitMetadata(
  import.meta.env.VITE_TROUTE_COMMIT_SHA,
);
