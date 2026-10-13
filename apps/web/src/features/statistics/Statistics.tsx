import { useMemo, useRef, useState, type ReactNode } from 'react';
import { Activity, ArrowRight, ArrowUpRight, AudioLines, CheckCircle2, Clock3, Flame, Gauge, Info, Keyboard, Lightbulb, RefreshCw, Sparkles, Timer, TrendingUp } from 'lucide-react';
import type { Recording } from '../../recorder';
import type { SessionAttempt } from '../home/statistics';
import { sessionStatistics, successfulJob, countWords } from '../home/statistics';
import { Dialog } from '../shell/Dialog';
import { Mark } from '../shell/Mark';
import { useEnumParam } from '../shell/urlState';
import { usePopover } from '../shell/usePopover';
import { hereLabel, languageLabel, modelName, textModelName } from '../home/language';
import { isTauri } from '../desktop-session/runtime/native';
import { useProcessingStatistics } from './useProcessingStatistics';
import { providerName } from '../transcription-setup/CloudKey';
import { Heatmap, LatencyStrip, Meter, Sparkline, WordsChart } from './charts';
import {
  activity, change, duration, heatmap, highlights, hostsFor, jobsFor, jobTime, languageShare, measured, percentile, previousRecordings,
  RANGES, selectedRecordings, speechModels, streaks, superAttempts, textModelActivity, textModelAdvice, textModelKey, textModelUse, timeSaved, timeSpent,
} from './metrics';
import './statistics.css';
import { LatencyExport } from './LatencyExport';

/** Series colours for the busiest text models; any further models share the last. */
const SERIES = 4;
const TYPING_KEY = 'writer-typing-wpm';
const readTyping = () => { try { const value = Number(localStorage.getItem(TYPING_KEY)); return value >= 10 && value <= 200 ? value : 40; } catch { return 40; } };

export function Statistics({ recordings, saved, loading, onOpenSession, onOpenSettings, onOpenHistory }: {
  recordings: Recording[];
  saved: Record<string, SessionAttempt>;
  loading: boolean;
  onOpenSession: (id: string) => void;
  /** Opens a Settings section, from a suggestion. */
  onOpenSettings?: (section: string) => void;
  /** Opens Home with a history filter, e.g. the failed dictations. */
  onOpenHistory?: (filter: 'attention') => void;
}) {
  const [range, setRange] = useEnumParam('period', RANGES, '7');
  const [selected, setSelected] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const [typing, setTyping] = useState(readTyping);
  const integrated = isTauri();
  const filtered = useMemo(() => selectedRecordings(recordings, range), [recordings, range]);
  const previous = useMemo(() => previousRecordings(recordings, range), [recordings, range]);
  const stats = useMemo(() => sessionStatistics(filtered, saved), [filtered, saved]);
  const before = useMemo(() => previous && sessionStatistics(previous, saved), [previous, saved]);
  const jobs = useMemo(() => filtered.flatMap(r => jobsFor(saved[r.id])), [filtered, saved]);
  const processing = useProcessingStatistics(jobs.filter(j => j.status.state === 'succeeded').map(j => j.id));
  const edits = superAttempts(Object.values(processing.attempts).flat());
  const completedEdits = edits.filter(a => a.state.state === 'succeeded' && !a.diagnostics?.original_used);
  const originalsDelivered = edits.filter(a => a.diagnostics?.original_used && a.diagnostics.delivery);
  const successful = jobs.filter(j => j.status.state === 'succeeded').length;
  const failed = jobs.filter(j => j.status.state === 'failed').length;
  const pending = jobs.length - successful - failed;
  const recordedMs = filtered.reduce((sum, r) => sum + (measured(r.duration) ? r.duration : 0), 0);
  const previousMs = previous?.reduce((sum, r) => sum + (measured(r.duration) ? r.duration : 0), 0) ?? null;
  const chartDays = range === '7' ? 7 : 30;
  const days = activity(filtered, saved, chartDays);
  const tokenEdits = completedEdits.filter(a => measured(a.diagnostics?.input_tokens) || measured(a.diagnostics?.output_tokens));
  const tokens = (key: 'input_tokens' | 'output_tokens') => {
    const values = completedEdits.flatMap(a => measured(a.diagnostics?.[key]) ? [a.diagnostics![key]!] : []);
    return values.length ? values.reduce((a, b) => a + b, 0).toLocaleString() : '—';
  };
  const measuredEdits = completedEdits.filter(a => a.diagnostics);
  const models = speechModels(jobs);
  const textModels = textModelUse(edits);
  const advice = textModelAdvice(edits);
  const hostsOf = (key: string) => hostsFor(edits.filter(a => textModelKey(a) === key)).filter(h => h.host);
  const series = (key: string) => Math.min(textModels.findIndex(m => m.key === key), SERIES - 1);
  const recordedAt = new Map(filtered.flatMap(r => jobsFor(saved[r.id]).map(j => [j.id, r.createdAt] as const)));
  const modelDays = textModelActivity(edits, recordedAt, chartDays);
  const maxEdits = Math.max(1, ...modelDays.map(d => d.total));
  const textName = (m: (typeof textModels)[number]) => m.selection ? textModelName(m.selection.model) : 'No text model';
  const textDetail = (m: (typeof textModels)[number]) => m.selection ? (m.selection.provider === 'local' ? 'On this computer' : providerName[m.selection.provider]) : '';
  const latencyScale = Math.min(10_000, Math.max(3200, ...textModels.map(m => (percentile(m.times, .9) ?? 0) * 1.1)));
  const currentPage = Math.min(page, Math.max(0, Math.ceil(filtered.length / 15) - 1));
  const detail = filtered.find(r => r.id === selected);
  const label = (date: Date) => date.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
  const languages = languageShare(filtered, saved);
  const streak = streaks(recordings);
  const facts = highlights(filtered, saved);
  const superShare = stats.transcribed ? Math.round(new Set(completedEdits.map(a => a.job_id)).size / stats.transcribed * 100) : null;
  const saving = timeSaved(stats.words, recordedMs, typing);
  const versus = range === '7' ? 'vs previous 7 days' : 'vs previous 30 days';
  const where = integrated ? `kept ${hereLabel()}` : 'kept by your server';
  const periodName = range === '7' ? 'The last 7 days' : range === '30' ? 'The last 30 days' : 'All history';
  const rate = successful + failed ? Math.round(successful / (successful + failed) * 100) : null;

  return (
    <main className="page statistics-page" aria-label="Statistics">
      <div className="scroller statistics-content">
        <div className="statistics-inner">
          <header className="statistics-head">
            <div>
              <h1><Mark size={22} /> Statistics</h1>
              <p>{periodName} · from {stats.recordings.toLocaleString()} dictation{stats.recordings === 1 ? '' : 's'} {where}</p>
            </div>
            <span className="spacer" />
            <div className="segmented" role="group" aria-label="Time period">
              {RANGES.map(id => (
                <button key={id} type="button" aria-pressed={range === id} onClick={() => { setRange(id); setPage(0); }}>
                  {id === '7' ? '7 days' : id === '30' ? '30 days' : 'All history'}
                </button>
              ))}
            </div>
          </header>

          <LatencyExport />
          {loading && <p className="notice" role="status">Loading saved history…</p>}
          {!loading && !filtered.length && (
            <div className="statistics-empty">
              <Activity size={24} aria-hidden />
              <div>
                <strong>{recordings.length ? 'No dictations in this period' : 'Your first dictation starts the picture'}</strong>
                <p>{recordings.length ? 'Choose a longer period to see your saved activity.' : 'Dictate on Home, then come back to see your words, your pace and your models.'}</p>
              </div>
            </div>
          )}

          <div className="tiles">
            <Tile hero icon={<AudioLines size={16} />} label="Words dictated" figure={stats.words.toLocaleString()} delta={change(stats.words, before?.words ?? null)} versus={versus} spark={days.map(d => d.words)} note="Counted once per dictation" />
            <Tile icon={<Clock3 size={16} />} label="Time dictating" figure={timeSpent(recordedMs)} delta={change(recordedMs, previousMs)} versus={versus} spark={days.map(d => d.ms)} note="All saved recordings" />
            <Tile icon={<Gauge size={16} />} label="Speaking pace" figure={stats.wordsPerMinute === null ? '—' : String(stats.wordsPerMinute)} unit={stats.wordsPerMinute === null ? undefined : 'wpm'} delta={change(stats.wordsPerMinute, before?.wordsPerMinute ?? null)} versus={versus} note="Including pauses" />
            <Tile
              icon={<Keyboard size={16} />}
              label="Time saved"
              figure={`≈ ${timeSpent(saving)}`}
              note={<TypingSpeed wpm={typing} onChange={(value) => { setTyping(value); try { localStorage.setItem(TYPING_KEY, String(value)); } catch { /* Kept for this visit. */ } }} />}
            />
          </div>

          <div className="statistics-grid">
            <section className="card" aria-label="Words per day">
              <CardHead title="Words per day" sub={`${range === 'all' ? 'The last 30 days' : periodName} · original transcripts · local dates`} />
              <WordsChart days={days} />
              <details className="statistics-daily">
                <summary>Daily totals</summary>
                <div className="statistics-daily-grid">
                  {days.map(d => <p key={d.day.getTime()}><span>{label(d.day)}</span><strong>{d.words.toLocaleString()} words</strong><span>{d.sessions} dictations</span></p>)}
                </div>
              </details>
            </section>
            <div className="statistics-stack">
              <section className="card" aria-label="Languages">
                <CardHead title="Languages" sub="Share of words this period" />
                <Meter label={languages.map(l => `${languageLabel(l.id)} ${l.share}%`).join(', ') || 'No words yet'} parts={languages.map((l, i) => ({ value: l.words, color: `var(--series-${i + 1})` }))} />
                <div className="legend">
                  {languages.length
                    ? languages.map((l, i) => <div key={l.id}><i style={{ background: `var(--series-${i + 1})` }} /><span dir="auto">{languageLabel(l.id)}</span><b>{l.share}%</b></div>)
                    : <p className="statistics-note">Languages appear after your first transcript.</p>}
                </div>
              </section>
              <section className="card streak-card" aria-label="Streak">
                <span className="streak-icon" aria-hidden><Flame size={24} /></span>
                <div>
                  <div className="streak-figure">{streak.current} {streak.current === 1 ? 'day' : 'days'} in a row</div>
                  <div className="card-sub">Your longest run in loaded history is {streak.longest} {streak.longest === 1 ? 'day' : 'days'}</div>
                </div>
              </section>
            </div>
          </div>

          <div className="statistics-grid">
            <section className="card" aria-label="When you dictate">
              <CardHead title="When you dictate" sub={`Words by hour of day · ${periodName.toLowerCase()}`} />
              <Heatmap grid={heatmap(filtered, saved)} />
            </section>
            <section className="card" aria-label="Highlights">
              <CardHead title="Highlights" sub="Facts from your history, nothing estimated" />
              <div className="facts">
                {facts.busiest && <Fact icon={<TrendingUp size={16} />}><b>{facts.busiest.day.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'short' })}</b> was your busiest day — {facts.busiest.words.toLocaleString()} words in {facts.busiest.sessions} dictation{facts.busiest.sessions === 1 ? '' : 's'}.</Fact>}
                {facts.longest && <Fact icon={<Timer size={16} />}>Your longest dictation was <b>{duration(facts.longest.duration)}</b> ({facts.longest.createdAt.toLocaleString(undefined, { weekday: 'long', hour: 'numeric', minute: '2-digit' })}).</Fact>}
                {superShare !== null && completedEdits.length > 0 && <Fact icon={<Sparkles size={16} />}><b>{superShare}%</b> of transcribed dictations were polished by Super.</Fact>}
                {!facts.busiest && !facts.longest && <p className="statistics-note">Highlights appear once there is something to tell.</p>}
              </div>
            </section>
          </div>

          <h2 className="statistics-section">Transcription</h2>
          <div className="statistics-grid thirds">
            <section className="card" aria-label="Reliability">
              <CardHead title={<><CheckCircle2 size={16} className="ok-icon" aria-hidden /> Reliability</>} sub="Finished transcription jobs, retries included" />
              <div className="big-figure">{rate === null ? '—' : `${rate}%`}</div>
              <Meter label={`${successful} succeeded, ${failed} failed`} parts={[{ value: successful, color: 'var(--success)' }, { value: failed, color: 'var(--danger)' }]} />
              <div className="legend">
                <div><i style={{ background: 'var(--success)' }} />Succeeded<b>{successful}</b></div>
                <div><i style={{ background: 'var(--danger)' }} />Failed — audio kept<b>{failed}</b></div>
                {pending > 0 && <div><i style={{ background: 'var(--border-strong)' }} />In progress<b>{pending}</b></div>}
                <div><i style={{ background: 'transparent' }} />Transcribed dictations<b>{stats.transcribed} / {stats.recordings}</b></div>
              </div>
              {failed > 0 && onOpenHistory && (
                <button type="button" className="row-link card-link" onClick={() => onOpenHistory('attention')}>Open failed dictations <ArrowRight size={14} aria-hidden /></button>
              )}
            </section>
            <section className="card span-2" aria-label="Speech models">
              <CardHead title="Speech models" sub="From saved recording to text, queueing included · median and 90% within" />
              {models.length ? (
                <div className="table-scroll">
                  <table className="stats-table">
                    <thead><tr><th>Model</th><th>Where</th><th className="n">Jobs</th><th className="n">Median</th><th className="n">90% within</th><th className="n">Succeeded</th></tr></thead>
                    <tbody>
                      {models.map(m => (
                        <tr key={m.key}>
                          <td><b dir="auto">{modelName(m.selection.model)}</b></td>
                          <td className="muted">{m.selection.provider === 'local' ? (integrated ? hereLabel() : 'on your server') : providerName[m.selection.provider] ?? m.selection.provider}</td>
                          <td className="n">{m.jobs}</td>
                          <td className="n">{duration(m.median)}</td>
                          <td className="n">{duration(m.p90)}</td>
                          <td className="n">{m.ended ? `${Math.round(m.succeeded / m.ended * 100)}%` : '—'}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              ) : <p className="statistics-note">Models appear after your first transcription attempt.</p>}
              <p className="statistics-note">Job time runs from submission to completion, including queueing. Jobs with optional cleanup are left out because they keep no separate transcription time. This is not stop-to-paste time.</p>
            </section>
          </div>

          <h2 className="statistics-section">Super polishing</h2>
          <div className="statistics-grid">
            <section className="card" aria-label="Text model statistics">
              <CardHead
                title="Text models"
                sub="Polishing time per model · the dot is the median, the bar reaches 90% of edits · the dashed line is 2.5 s"
                action={<button type="button" className="icon-button" aria-label="Refresh processing statistics" disabled={processing.loading} onClick={processing.reload}><RefreshCw size={16} aria-hidden /></button>}
              />
              {processing.loading && <p className="statistics-note" role="status">Loading processing measurements…</p>}
              {!!processing.failed.length && <p className="notice warn" role="status">Processing history unavailable for {processing.failed.length} jobs. Totals below are partial. <button type="button" className="row-link" onClick={processing.reload}>Retry</button></p>}
              {textModels.length ? (
                <ul className="latency-list statistics-text-models">
                  {textModels.map(m => {
                    const ended = m.succeeded + m.failed;
                    const median = percentile(m.times, .5);
                    const p90 = percentile(m.times, .9);
                    return (
                      <li key={m.key} className="latency-row">
                        <div className="latency-name">
                          <strong dir="auto">{textName(m)}</strong>
                          <span>{textDetail(m) && `${textDetail(m)} · `}{m.succeeded} successful / {m.attempts} {m.attempts === 1 ? 'edit' : 'edits'}{ended ? ` · ${Math.round(m.succeeded / ended * 100)}% succeeded` : ''}</span>
                        </div>
                        <LatencyStrip median={median} p90={p90} scale={latencyScale} color={`var(--series-${series(m.key) + 1})`} />
                        <dl className="latency-figures">
                          <div><dt>Typical</dt><dd>{duration(median)}</dd></div>
                          <div><dt>90% within</dt><dd>{duration(p90)}</dd></div>
                        </dl>
                        {/* Which host answered is the difference between a fast dictation
                            and a slow one when one model is served by many of them. */}
                        {hostsOf(m.key).length > 0 && <p className="statistics-hosts">{hostsOf(m.key).map(h => `${h.host}: ${duration(h.median)} over ${h.edits}`).join(' · ')}</p>}
                      </li>
                    );
                  })}
                </ul>
              ) : !processing.loading && <p className="statistics-note">Text models appear after your first Super edit in this period.</p>}
              <p className="statistics-note">Times cover {textModels.reduce((n, m) => n + m.times.length, 0)} successful model requests; edits that reused an early draft are left out. Primary and fallback calls count under the model that actually ran. Using the original is not counted as successful polishing.</p>
            </section>
            <section className="card" aria-label="Suggestions">
              <CardHead title="Suggestions" sub="Only from your own measurements" />
              {advice.length ? (
                <ul className="insights" aria-label="Text model suggestions">
                  {advice.map(line => (
                    <li key={line} className="insight">
                      <span className="insight-icon" aria-hidden><Lightbulb size={16} /></span>
                      <div>{line}</div>
                    </li>
                  ))}
                </ul>
              ) : <p className="statistics-note">Nothing to change: no model is slow or failing often enough to suggest anything.</p>}
              {onOpenSettings && <button type="button" className="row-link card-link" onClick={() => onOpenSettings('modes')}>Open Modes <ArrowRight size={14} aria-hidden /></button>}
              <p className="statistics-note with-icon"><Info size={14} aria-hidden /> Polishing time never includes transcription time; early drafts reused while you spoke are not counted.</p>
            </section>
          </div>

          <details className="card statistics-more">
            <summary>All measurements <span>· sessions, tokens and daily model use</span></summary>
            <div className="statistics-more-body">
              <section aria-label="Super mode statistics">
                <h3>Super mode</h3>
                <dl className="statistics-facts">
                  <div><dt>Successful edits</dt><dd>{completedEdits.length}</dd></div>
                  <div><dt>Original used</dt><dd>{edits.filter(a => a.diagnostics?.original_used).length}</dd></div>
                  <div><dt>Failed / in progress</dt><dd>{edits.filter(a => a.state.state === 'failed').length} / {edits.filter(a => a.state.state === 'pending').length}</dd></div>
                  <div><dt>Early drafts reused</dt><dd>{measuredEdits.length ? `${measuredEdits.filter(a => a.diagnostics!.reused_draft).length} / ${measuredEdits.length}` : '—'}</dd></div>
                  <div><dt>Input / output tokens</dt><dd>{tokens('input_tokens')} / {tokens('output_tokens')}</dd></div>
                </dl>
                <p className="statistics-note">Token counts cover {tokenEdits.length} successful edits with reported usage. Missing measurements are left out, never counted as zero.</p>
                {originalsDelivered.length > 0 && <p className="statistics-note">Original delivered: {originalsDelivered.filter(a => a.diagnostics!.delivery!.pasted).length} inserted · {originalsDelivered.filter(a => a.diagnostics!.delivery!.copied && !a.diagnostics!.delivery!.pasted).length} copied. Typical stop to delivery: {duration(percentile(originalsDelivered.flatMap(a => measured(a.diagnostics?.delivery?.stop_to_delivery_ms) ? [a.diagnostics.delivery.stop_to_delivery_ms] : []), .5))}.</p>}
              </section>

              {textModels.length > 0 && (
                <section aria-label="Daily model use">
                  <h3>Text model use per day</h3>
                  <div className="model-activity-chart" role="img" aria-label={`Polishing attempts per day by text model. ${modelDays.reduce((n, d) => n + d.total, 0)} attempts over ${modelDays.length} days. Exact values in Daily model use below.`}>
                    {modelDays.map(d => (
                      <div className="activity-day" key={d.day.getTime()} title={`${label(d.day)}: ${d.total ? textModels.filter(m => d.counts.has(m.key)).map(m => `${textName(m)} ${d.counts.get(m.key)}`).join(', ') : 'no edits'}`}>
                        <div className="model-activity-stack" style={{ height: `${d.total / maxEdits * 100}%` }}>
                          {textModels.filter(m => d.counts.has(m.key)).map(m => <i key={m.key} style={{ flexGrow: d.counts.get(m.key), background: `var(--series-${series(m.key) + 1})` }} />)}
                        </div>
                      </div>
                    ))}
                  </div>
                  <div className="activity-axis"><span>{label(modelDays[0].day)}</span><span>{label(modelDays[modelDays.length - 1].day)}</span></div>
                  <details className="statistics-daily">
                    <summary>Daily model use</summary>
                    <div className="statistics-daily-grid">
                      {modelDays.filter(d => d.total).map(d => <p key={d.day.getTime()}><span>{label(d.day)}</span><strong>{textModels.filter(m => d.counts.has(m.key)).map(m => `${textName(m)} ${d.counts.get(m.key)}`).join(' · ')}</strong></p>)}
                      {!modelDays.some(d => d.total) && <p><span>No polishing attempts in the chart’s days.</span></p>}
                    </div>
                  </details>
                </section>
              )}

              <section className="statistics-sessions" aria-label="Session timings">
                <h3>Dictation by dictation <span>{filtered.length} dictations</span></h3>
                <div className="table-scroll">
                  <table className="stats-table">
                    <thead><tr><th>Recorded</th><th className="n">Words</th><th className="n">Audio</th><th className="n">Latest successful job</th><th>Status</th><th><span className="visually-hidden">Actions</span></th></tr></thead>
                    <tbody>
                      {filtered.slice(currentPage * 15, currentPage * 15 + 15).map(r => {
                        const job = successfulJob(saved[r.id]);
                        return (
                          <tr key={r.id}>
                            <td>{r.createdAt.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })}</td>
                            <td className="n">{job?.status.state === 'succeeded' ? countWords(job.status.result.transcript.raw_text) : '—'}</td>
                            <td className="n">{duration(r.duration)}</td>
                            <td className="n">{job ? duration(jobTime(job)) : '—'}</td>
                            <td>{job ? 'Transcribed' : saved[r.id]?.job?.status.state === 'failed' ? 'Failed' : jobsFor(saved[r.id]).length ? 'In progress' : 'Saved audio'}</td>
                            <td><button type="button" className="row-link" onClick={() => setSelected(r.id)}>Details <ArrowUpRight size={14} aria-hidden /></button></td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </div>
                {!filtered.length && <p className="statistics-note">No dictations to show.</p>}
                {filtered.length > 15 && (
                  <div className="statistics-pagination">
                    <button type="button" className="button small" disabled={currentPage === 0} onClick={() => setPage(currentPage - 1)}>Previous</button>
                    <span>Page {currentPage + 1} of {Math.ceil(filtered.length / 15)}</span>
                    <button type="button" className="button small" disabled={(currentPage + 1) * 15 >= filtered.length} onClick={() => setPage(currentPage + 1)}>Next</button>
                  </div>
                )}
              </section>
            </div>
          </details>
          <p className="statistics-note statistics-scope">Based on loaded, retained history. Deleted dictations are not included. The period filters by recording date; its retries and edits are included with that dictation.</p>
        </div>
      </div>

      {detail && (
        <Dialog title="Session timings" onClose={() => setSelected(null)}>
          <div className="statistics-session-detail">
            <p>{detail.createdAt.toLocaleString()} · {duration(detail.duration)} of audio</p>
            {jobsFor(saved[detail.id]).map(job => (
              <section key={job.id}>
                <h3>{modelName(job.request.options.selection.model)}</h3>
                <p>{job.status.state} · {new Date(job.created_at_ms).toLocaleString()}</p>
                <dl className="statistics-facts"><div><dt>Transcription job</dt><dd>{duration(jobTime(job))}</dd></div></dl>
                {job.audio_preparation && <dl className="statistics-facts">
                  <div><dt>Audio sent to the model</dt><dd>{duration(job.audio_preparation.input_ms)} → {duration(job.audio_preparation.output_ms)}</dd></div>
                  <div><dt>Silence shortened</dt><dd>{duration(job.audio_preparation.removed_ms)}</dd></div>
                  <div><dt>Audio size</dt><dd>{(job.audio_preparation.input_bytes / 1_000_000).toFixed(2)} → {(job.audio_preparation.output_bytes / 1_000_000).toFixed(2)} MB</dd></div>
                  <div><dt>Audio preparation</dt><dd>{duration(job.audio_preparation.preparation_ms)}</dd></div>
                </dl>}
                {(processing.attempts[job.id] ?? []).map(a => (
                  <div className="statistics-edit" key={a.id}>
                    <h4>{a.profile.name} · {a.state.state}{a.profile.text_model && <> · <span dir="auto">{textModelName(a.profile.text_model.model)}</span></>}</h4>
                    <dl className="statistics-facts">
                      <div><dt>Processing</dt><dd>{measured(a.diagnostics?.processing_ms) ? duration(a.diagnostics!.processing_ms) : 'Not recorded'}</dd></div>
                      <div><dt>First response / complete response</dt><dd>{duration(a.diagnostics?.first_response_ms)} / {duration(a.diagnostics?.response_complete_ms)}</dd></div>
                      <div><dt>Early draft reused</dt><dd>{a.diagnostics ? a.diagnostics.reused_draft ? 'Yes' : 'No' : 'Not recorded'}</dd></div>
                      <div><dt>Input / output tokens</dt><dd>{a.diagnostics?.input_tokens ?? '—'} / {a.diagnostics?.output_tokens ?? '—'}</dd></div>
                    </dl>
                  </div>
                ))}
                {processing.failed.includes(job.id) && <p>Processing measurements unavailable. Close and retry loading them.</p>}
              </section>
            ))}
            {!jobsFor(saved[detail.id]).length && <p>This recording has no saved transcription attempts.</p>}
            <p className="statistics-note">Processing stages may overlap. A dash means no completed measurement was recorded.</p>
            <button type="button" className="button primary" onClick={() => { setSelected(null); onOpenSession(detail.id); }}>Open in history <ArrowUpRight size={15} aria-hidden /></button>
          </div>
        </Dialog>
      )}
    </main>
  );
}

function CardHead({ title, sub, action }: { title: ReactNode; sub?: string; action?: ReactNode }) {
  return (
    <div className="card-head">
      <div>
        <h3>{title}</h3>
        {sub && <div className="card-sub">{sub}</div>}
      </div>
      {action && <><span className="spacer" />{action}</>}
    </div>
  );
}

function Tile({ icon, label, figure, unit, delta, versus, spark, note, hero }: {
  icon: ReactNode;
  label: string;
  figure: string;
  unit?: string;
  delta?: number | null;
  versus?: string;
  spark?: number[];
  note?: ReactNode;
  hero?: boolean;
}) {
  const shown = delta !== undefined && delta !== null;
  return (
    <section className={`tile${hero ? ' hero' : ''}`} aria-label={label}>
      <div className="tile-label">{icon}{label}</div>
      <div className="tile-figure">{figure}{unit && <small>{unit}</small>}</div>
      <div className="tile-foot">
        {shown
          ? delta === 0
            ? <>Same as {versus?.replace(/^vs /, '')}</>
            : <><span className={`delta ${delta! > 0 ? 'up' : 'down'}`}>{delta! > 0 ? '▲' : '▼'} {Math.abs(delta!)}%</span>{versus}</>
          : note}
      </div>
      {spark && <Sparkline values={spark} />}
    </section>
  );
}

function Fact({ icon, children }: { icon: ReactNode; children: ReactNode }) {
  return <div className="insight fact"><span className="insight-icon quiet" aria-hidden>{icon}</span><div>{children}</div></div>;
}

/** "estimate vs typing at 40 wpm", where 40 is yours to change. */
function TypingSpeed({ wpm, onChange }: { wpm: number; onChange: (wpm: number) => void }) {
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  const panel = useRef<HTMLDivElement>(null);
  usePopover(open, panel, anchor, () => setOpen(false));
  return (
    <span className="typing-speed">
      Estimate vs typing at{' '}
      <button ref={anchor} type="button" className="row-link" aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen(!open)}>{wpm} wpm</button>
      {open && (
        <div className="popover typing-popover" role="dialog" aria-label="Typing speed" ref={panel}>
          <label className="field">
            <span>How fast you type</span>
            <input type="number" min={10} max={200} step={5} defaultValue={wpm} onChange={(event) => { const value = Number(event.target.value); if (value >= 10 && value <= 200) onChange(value); }} />
          </label>
          <p>Most people type 35–45 words a minute. Time saved is the typing time for your words, minus the time you spent speaking them.</p>
        </div>
      )}
    </span>
  );
}
