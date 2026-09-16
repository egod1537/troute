import {
  Button,
  Menu,
  MenuItem,
  PopoverAnimation,
  PopoverNext,
} from "@blueprintjs/core";
import { SunIcon } from "@blueprintjs/icons/lib/esm/next/generated/components/sun";
import type { ThemeMode } from "./theme";

interface ThemeControlProps {
  mode: ThemeMode;
  onChange: (mode: ThemeMode) => void;
}

const LABELS: Record<ThemeMode, string> = {
  system: "시스템",
  light: "라이트",
  dark: "다크",
};

function themeIcon(mode: ThemeMode) {
  if (mode === "light") return <SunIcon size={16} />;
  return mode === "dark" ? "moon" : "desktop";
}

export function ThemeControl({ mode, onChange }: ThemeControlProps) {
  const menu = (
    <Menu aria-label="테마">
      {(["system", "light", "dark"] as const).map((option) => (
        <MenuItem
          active={mode === option}
          icon={themeIcon(option)}
          key={option}
          onClick={() => onChange(option)}
          text={LABELS[option]}
        />
      ))}
    </Menu>
  );

  return (
    <PopoverNext
      animation={PopoverAnimation.MINIMAL}
      arrow={false}
      content={menu}
      placement="bottom-end"
      inheritDarkTheme
    >
      <Button
        aria-label={`테마: ${LABELS[mode]}`}
        title="테마 변경"
        icon={themeIcon(mode)}
        variant="minimal"
      />
    </PopoverNext>
  );
}
