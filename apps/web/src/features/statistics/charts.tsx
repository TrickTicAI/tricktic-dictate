import { useId, useState } from 'react';
import { niceMax } from './metrics';

/**
 * Charts for Statistics, drawn with the theme: the accent and the accent
 * rotated around the colour wheel (`--series-*`), never the success, warning
 * or danger colours, which mean something else in this application.
 */

/** A small trend line for a tile. Decorative: the figure beside it is the value. */
export function Sparkline({ values, width = 96, height = 32 }: { values: number[]; width?: number; height?: number }) {
  const gradient = useId();
  if (values.length < 2 || !values.some((value) => value > 0)) return null;
  const max = Math.max(...values, 1);
  const points = values.map((value, index) => [index * (width / (values.length - 1)), height - 3 - (value / max) * (height - 6)]);
  const line = points.map((point, index) => `${index ? 'L' : 'M'}${point[0].toFixed(1)} ${point[1].toFixed(1)}`).join(' ');
  const last = points[points.length - 1];
  return (
    <svg width={width} height={height} className="sparkline" aria-hidden focusable="false">
      <defs>
        <linearGradient id={gradient} x1="0" x2="0" y1="0" y2="1">
          <stop offset="0" stopColor="var(--accent)" stopOpacity=".28" />
          <stop offset="1" stopColor="var(--accent)" stopOpacity="0" />
        </linearGradient>
      </defs>
      <path d={`${line} L${width} ${height} L0 ${height} Z`} fill={`url(#${gradient})`} />
      <path d={line} fill="none" stroke="var(--accent)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
      <circle cx={last[0]} cy={last[1]} r="3" fill="var(--accent)" />
    </svg>
  );
}

export interface ChartDay { day: Date; words: number; sessions: number }

const short = (date: Date) => date.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' });

/**
 * Words per day as a real chart: a scale, gridlines, today in full accent and
 * every day reachable by pointer or keyboard, each with its own tooltip.
 */
export function WordsChart({ days }: { days: ChartDay[] }) {
  const [active, setActive] = useState<number | null>(null);
  const max = niceMax(Math.max(...days.map((day) => day.words)));
  const week = days.length <= 7;
  const total = days.reduce((sum, day) => sum + day.words, 0);
  return (
    <div className="words-chart" role="group" aria-label={`Words per day. ${total.toLocaleString()} words over ${days.length} days.`}>
      <div className="chart-y" aria-hidden>
        <span>{max.toLocaleString()}</span>
        <span>{(max / 2).toLocaleString()}</span>
        <span>0</span>
      </div>
      <div className="chart-plot">
        <div className="chart-grid" aria-hidden><i /><i /><i /></div>
        {days.map((day, index) => {
          const today = index === days.length - 1;
          const label = week
            ? `${day.day.toLocaleDateString(undefined, { weekday: 'short' })}${today ? ' · today' : ''}`
            : index % 5 === 4 || today ? String(day.day.getDate()) : '';
          return (
            <div
              key={day.day.getTime()}
              className={`chart-bar${today ? ' today' : ''}${day.words ? '' : ' zero'}`}
              style={{ height: `${(day.words / max) * 100}%` }}
              tabIndex={0}
              aria-label={`${short(day.day)}: ${day.words.toLocaleString()} words in ${day.sessions} dictation${day.sessions === 1 ? '' : 's'}`}
              onMouseEnter={() => setActive(index)}
              onMouseLeave={() => setActive(null)}
              onFocus={() => setActive(index)}
              onBlur={() => setActive(null)}
            >
              {label && <span className="chart-x" aria-hidden>{label}</span>}
              {active === index && (
                <span className="chart-tip" role="tooltip">
                  <b>{day.words.toLocaleString()} words</b> · {short(day.day)} · {day.sessions} dictation{day.sessions === 1 ? '' : 's'}
                </span>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/* Monday first; names come from the locale. 2024-01-01 was a Monday. */
const WEEKDAYS = Array.from({ length: 7 }, (_, index) => new Date(2024, 0, 1 + index).toLocaleDateString(undefined, { weekday: 'short' }));

/** Words by weekday and hour: when you dictate, at a glance. */
export function Heatmap({ grid }: { grid: number[][] }) {
  const max = Math.max(1, ...grid.flat());
  let peak = { day: 0, hour: 0, words: 0 };
  grid.forEach((row, day) => row.forEach((words, hour) => { if (words > peak.words) peak = { day, hour, words }; }));
  return (
    <div
      className="heatmap"
      role="img"
      aria-label={peak.words ? `Words by weekday and hour. The busiest hour is ${WEEKDAYS[peak.day]} ${peak.hour}:00 with ${peak.words} words.` : 'No words yet in this period.'}
    >
      {grid.map((row, day) => (
        <div key={day} className="heat-row">
          <span className="heat-label">{WEEKDAYS[day]}</span>
          {row.map((words, hour) => (
            <i
              key={hour}
              title={`${WEEKDAYS[day]} ${hour}:00 · ${words} words`}
              style={words ? { background: `color-mix(in srgb, var(--accent) ${Math.round(14 + (words / max) * 86)}%, var(--bg-hover))` } : undefined}
            />
          ))}
        </div>
      ))}
      <div className="heat-row heat-hours" aria-hidden>
        <span />
        {Array.from({ length: 24 }, (_, hour) => <span key={hour}>{hour % 6 === 0 ? `${hour}:00` : ''}</span>)}
      </div>
    </div>
  );
}

/** A proportion bar, with a gap between parts. */
export function Meter({ parts, label }: { parts: { value: number; color: string }[]; label: string }) {
  const total = parts.reduce((sum, part) => sum + part.value, 0);
  return (
    <div className="meter" role="img" aria-label={label}>
      {total > 0 && parts.filter((part) => part.value > 0).map((part, index) => (
        <i key={index} style={{ width: `${(part.value / total) * 100}%`, background: part.color }} />
      ))}
    </div>
  );
}

/**
 * One text model's polishing time: the dot is the median, the bar reaches the
 * time 90% of edits finished within, the dashed line is the 2.5 s budget a
 * paste should stay under.
 */
export function LatencyStrip({ median, p90, scale, limit = 2500, color }: { median: number | null; p90: number | null; scale: number; limit?: number; color: string }) {
  const at = (ms: number) => `${Math.min(100, (ms / scale) * 100)}%`;
  return (
    <span className="latency-track" style={{ '--series-c': color } as React.CSSProperties} aria-hidden>
      <span className="latency-axis" />
      {median !== null && p90 !== null && <span className="latency-whisker" style={{ insetInlineStart: at(median), width: `calc(${at(p90)} - ${at(median)})` }} />}
      {median !== null && <span className="latency-median" style={{ insetInlineStart: at(median) }} />}
      <span className="latency-limit" style={{ insetInlineStart: at(limit) }} />
    </span>
  );
}
