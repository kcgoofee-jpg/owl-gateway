/** 工作台自有图标。不引用 NovelAI 字标，顶栏用本站猫头鹰眼睛。 */

type IconProps = { size?: number; class?: string };

export function OwlIcon({ size = 30, class: className }: IconProps) {
  return (
    <svg class={className} width={size} height={size} viewBox="0 0 32 32" fill="none" aria-hidden="true">
      <path
        d="m5 8 4 1.5 3-3L16 8l4-1.5 3 3L27 8v8.8c0 6.1-4 10.2-11 12.7-7-2.5-11-6.6-11-12.7V8Z"
        fill="currentColor"
      />
      <path d="m10 7-1-3 4 2m6 0 4-2-1 3" stroke="currentColor" stroke-width="2" stroke-linejoin="round" />
      <circle cx="12" cy="15" r="4.1" fill="var(--nai-panel, #111322)" />
      <circle cx="20" cy="15" r="4.1" fill="var(--nai-panel, #111322)" />
      <circle cx="12" cy="15" r="1.4" fill="var(--nai-eye, #ffc53d)" />
      <circle cx="20" cy="15" r="1.4" fill="var(--nai-eye, #ffc53d)" />
      <path d="m16 17.2-2 3h4l-2-3Z" fill="var(--nai-panel, #111322)" />
    </svg>
  );
}

export function MenuIcon({ size = 18 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path d="M4 7h16M4 12h16M4 17h16" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
    </svg>
  );
}

export function PlusIcon({ size = 18 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path d="M12 5v14M5 12h14" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
    </svg>
  );
}

export function SparkleIcon({ size = 16 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M12 2.5 13.7 8.3 19.5 10 13.7 11.7 12 17.5 10.3 11.7 4.5 10 10.3 8.3 12 2.5Z" />
    </svg>
  );
}

export function SwapIcon({ size = 14 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path
        d="M7 7h11l-3-3M17 17H6l3 3"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

export function PortraitIcon({ size = 18 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <rect x="8.5" y="3.5" width="7" height="17" rx="1.5" stroke="currentColor" stroke-width="1.6" />
    </svg>
  );
}

export function LandscapeIcon({ size = 18 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <rect x="3" y="7" width="18" height="10" rx="2" stroke="currentColor" stroke-width="2" />
    </svg>
  );
}

export function SquareIcon({ size = 18 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <rect x="5" y="5" width="14" height="14" rx="2" stroke="currentColor" stroke-width="2" />
    </svg>
  );
}

export function ChevronIcon({ size = 16, open = false }: IconProps & { open?: boolean }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
      style={{ transform: open ? "rotate(90deg)" : undefined }}
    >
      <path d="M9 6l6 6-6 6" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
    </svg>
  );
}

export function HistoryIcon({ size = 22 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path
        d="M12 7.5V12l2.6 1.6"
        stroke="currentColor"
        stroke-width="1.8"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
      <path d="M5.2 8.2A8 8 0 1 1 4.2 13" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
      <path
        d="M4.2 5.2v3.4h3.4"
        stroke="currentColor"
        stroke-width="1.8"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

export function SeedIcon({ size = 16 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path
        d="M12 20c0-5 3.2-8 7-9.2C15 15 13.5 18 12 20Z"
        stroke="currentColor"
        stroke-width="1.6"
        stroke-linejoin="round"
      />
      <path
        d="M12 20C12 14 8.2 11 4 10c3.2 4 5.2 7 8 10Z"
        stroke="currentColor"
        stroke-width="1.6"
        stroke-linejoin="round"
      />
      <path d="M12 20V11" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
    </svg>
  );
}

export function AnlasIcon({ size = 14 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M12 2 15 9l7 3-7 3-3 7-3-7-7-3 7-3 3-7Z" />
    </svg>
  );
}

export function PlayIcon({ size = 16 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <path d="M8 5.5v13l11-6.5-11-6.5Z" />
    </svg>
  );
}

export function ImageIcon({ size = 22 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <rect x="3" y="5" width="18" height="14" rx="2" stroke="currentColor" stroke-width="2" />
      <circle cx="8.5" cy="10" r="1.5" fill="currentColor" />
      <path d="M21 16l-5.5-5.5L7 19" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
    </svg>
  );
}

export function MaskIcon({ size = 18 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path d="M4 8h4l2-3h4l2 3h4v11H4V8Z" stroke="currentColor" stroke-width="2" stroke-linejoin="round" />
      <circle cx="12" cy="13" r="3" stroke="currentColor" stroke-width="2" />
    </svg>
  );
}

export function InpaintIcon({ size = 18 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path d="M4 20h7L20 11l-4-4L7 16v4Z" stroke="currentColor" stroke-width="2" stroke-linejoin="round" />
    </svg>
  );
}

export function DiceIcon({ size = 16 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <rect x="4" y="4" width="16" height="16" rx="3" stroke="currentColor" stroke-width="2" />
      <circle cx="9" cy="9" r="1.2" fill="currentColor" />
      <circle cx="15" cy="15" r="1.2" fill="currentColor" />
      <circle cx="15" cy="9" r="1.2" fill="currentColor" />
    </svg>
  );
}

export function RefreshIcon({ size = 16 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path
        d="M20 12a8 8 0 1 1-2.3-5.6M20 5v5h-5"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

export function CloseIcon({ size = 14 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path d="M6 6l12 12M18 6 6 18" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
    </svg>
  );
}

export function TokenIcon({ size = 16 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <rect x="4" y="4" width="6" height="6" rx="1.2" stroke="currentColor" stroke-width="2" />
      <rect x="14" y="4" width="6" height="6" rx="1.2" stroke="currentColor" stroke-width="2" />
      <rect x="4" y="14" width="6" height="6" rx="1.2" stroke="currentColor" stroke-width="2" />
      <rect x="14" y="14" width="6" height="6" rx="1.2" stroke="currentColor" stroke-width="2" />
    </svg>
  );
}

export function PinIcon({ size = 14 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path d="M12 21s7-5.2 7-11a7 7 0 1 0-14 0c0 5.8 7 11 7 11Z" stroke="currentColor" stroke-width="2" />
      <circle cx="12" cy="10" r="2.2" stroke="currentColor" stroke-width="2" />
    </svg>
  );
}

export function CopyIcon({ size = 14 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <rect x="8" y="8" width="12" height="12" rx="2" stroke="currentColor" stroke-width="2" />
      <path
        d="M6 16H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"
        stroke="currentColor"
        stroke-width="2"
      />
    </svg>
  );
}

export function ExpandIcon({ size = 16 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path
        d="M9 4H4v5M15 4h5v5M9 20H4v-5M15 20h5v-5"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
      />
    </svg>
  );
}

export function CheckIcon({ size = 14 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path
        d="M5 12.5 10 17l9-10"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

export function TrashIcon({ size = 14 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path
        d="M5 7h14M10 7V5h4v2M8 7l1 12h6l1-12"
        stroke="currentColor"
        stroke-width="2"
        stroke-linejoin="round"
      />
    </svg>
  );
}

export function ArrowIcon({ size = 12, dir = "up" }: IconProps & { dir?: "up" | "down" }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
      style={{ transform: dir === "down" ? "rotate(180deg)" : undefined }}
    >
      <path
        d="M6 14l6-6 6 6"
        stroke="currentColor"
        stroke-width="2"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}

export function KebabIcon({ size = 16 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
      <circle cx="12" cy="6" r="1.6" />
      <circle cx="12" cy="12" r="1.6" />
      <circle cx="12" cy="18" r="1.6" />
    </svg>
  );
}

export function UploadIcon({ size = 16 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <path d="M12 16V7M8 10l4-4 4 4M5 19h14" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
    </svg>
  );
}

export function VibeIcon({ size = 22 }: IconProps) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden="true">
      <rect x="4" y="5" width="12" height="14" rx="2" stroke="currentColor" stroke-width="2" />
      <path d="M16 8h4v11a2 2 0 0 1-2 2h-8" stroke="currentColor" stroke-width="2" />
    </svg>
  );
}
