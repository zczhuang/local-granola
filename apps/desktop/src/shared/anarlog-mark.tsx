export function AnarlogMark({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 200 200"
      fill="none"
      className={className}
      aria-hidden="true"
    >
      <ellipse
        cx="100"
        cy="100"
        rx="66"
        ry="72"
        stroke="currentColor"
        strokeWidth="28"
      />
      <ellipse
        cx="100"
        cy="100"
        rx="50"
        ry="56"
        stroke="#D9A05B"
        strokeWidth="3.5"
        opacity="0.7"
      />
    </svg>
  );
}
