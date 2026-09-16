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
  system: "System",
  light: "Light",
  dark: "Dark",
};

function themeIcon(mode: ThemeMode) {
  if (mode === "light") return <SunIcon size={16} />;
  return mode === "dark" ? "moon" : "desktop";
}

export function ThemeControl({ mode, onChange }: ThemeControlProps) {
  const menu = (
    <Menu aria-label="Theme">
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
        aria-label={`Theme: ${LABELS[mode]}`}
        title={`Theme: ${LABELS[mode]}`}
        icon={themeIcon(mode)}
        variant="minimal"
      />
    </PopoverNext>
  );
}
