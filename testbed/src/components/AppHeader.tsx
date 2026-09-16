import {
  Alignment,
  Button,
  Classes,
  Intent,
  Navbar,
  NavbarDivider,
  NavbarGroup,
  NavbarHeading,
  Tag,
} from "@blueprintjs/core";
import { API_BASE, ROUTE_PATH } from "../api";
import { BUILD_COMMIT } from "../commit";
import { ThemeControl } from "../ThemeControl";
import type { ThemeMode } from "../theme";
import { TrouteIcon } from "../TrouteIcon";

export type HealthState = "checking" | "online" | "offline";

function healthIntent(health: HealthState) {
  if (health === "online") return Intent.SUCCESS;
  if (health === "offline") return Intent.DANGER;
  return Intent.PRIMARY;
}

function CommitIndicator() {
  const commitLabel = BUILD_COMMIT.fullSha
    ? BUILD_COMMIT.shortSha
    : "알 수 없음";
  const tag = (
    <Tag
      aria-label={`commit ${commitLabel}`}
      className={`${Classes.MONOSPACE_TEXT} navbar-commit`}
      icon="git-commit"
      minimal
      title={
        BUILD_COMMIT.fullSha
          ? `commit ${BUILD_COMMIT.fullSha}`
          : "commit 알 수 없음"
      }
    >
      <span className="navbar-commit-prefix">commit </span>
      {commitLabel}
    </Tag>
  );

  if (!BUILD_COMMIT.url) return tag;
  return (
    <a
      aria-label={`commit ${BUILD_COMMIT.fullSha}`}
      className="navbar-commit-link"
      href={BUILD_COMMIT.url}
      target="_blank"
      rel="noreferrer"
      title={`commit ${BUILD_COMMIT.fullSha}`}
    >
      {tag}
    </a>
  );
}

interface AppHeaderProps {
  health: HealthState;
  healthRefreshing: boolean;
  themeMode: ThemeMode;
  onRefreshHealth: () => void;
  onThemeChange: (mode: ThemeMode) => void;
}

export function AppHeader({
  health,
  healthRefreshing,
  themeMode,
  onRefreshHealth,
  onThemeChange,
}: AppHeaderProps) {
  const healthLabel =
    health === "online"
      ? "정상"
      : health === "offline"
        ? "연결 실패"
        : "확인 중";

  return (
    <Navbar className="app-navbar">
      <NavbarGroup align={Alignment.START}>
        <NavbarHeading className="navbar-brand">
          <TrouteIcon />
          <span>troute 테스트베드</span>
        </NavbarHeading>
        <NavbarDivider />
        <code
          className={`${Classes.MONOSPACE_TEXT} ${Classes.TEXT_MUTED} navbar-endpoint`}
        >
          {API_BASE} · {ROUTE_PATH ? `POST ${ROUTE_PATH}` : "GET /health 전용"}
        </code>
        <NavbarDivider />
        <CommitIndicator />
      </NavbarGroup>
      <NavbarGroup align={Alignment.END}>
        <span className={`${Classes.TEXT_MUTED} navbar-api-label`}>API</span>
        <Tag
          aria-label={`API ${healthLabel}`}
          icon={
            health === "online"
              ? "tick-circle"
              : health === "offline"
                ? "error"
                : "time"
          }
          intent={healthIntent(health)}
          minimal
        >
          {healthLabel}
        </Tag>
        <Button
          aria-label="API 상태 새로고침"
          title="API 상태 새로고침"
          icon="refresh"
          loading={healthRefreshing}
          disabled={healthRefreshing}
          variant="minimal"
          onClick={onRefreshHealth}
        />
        <ThemeControl mode={themeMode} onChange={onThemeChange} />
      </NavbarGroup>
    </Navbar>
  );
}
