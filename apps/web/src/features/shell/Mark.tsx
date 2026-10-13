/** The product mark: the seven-bar waveform from the app icon, in the accent. */
export function Mark({ size = 18 }: { size?: number }) {
  const bars = [0.38, 0.7, 1, 0.62, 0.86, 0.5, 0.3];
  return (
    <svg className="mark" width={size * 1.1} height={size} viewBox="0 0 26.5 20" aria-hidden focusable="false">
      {bars.map((h, i) => <rect key={i} x={i * 4} y={10 - h * 9} width={2.5} height={h * 18} rx={1.25} fill="currentColor" />)}
    </svg>
  );
}
