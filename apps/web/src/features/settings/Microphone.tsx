import { useEffect, useState } from 'react';
import { isTauri } from '../desktop-session/runtime/native';
import { listMicrophones, preferBuiltInMicrophone, selectMicrophone, type MicrophoneList } from '../recorder/microphones';
import { Row, Toggle } from './controls';

export function Microphone({ disabled }: { disabled: boolean }) {
  const [list, setList] = useState<MicrophoneList>({ devices: [], selected_id: null });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(true);
  const message = (error: unknown) => error instanceof Error ? error.message : typeof error === 'string' ? error : 'Cannot access microphone settings.';
  useEffect(() => {
    let disposed = false;
    const refresh = () => {
      void listMicrophones().then(value => { if (!disposed) { setList(value); setError(null); } })
        .catch(error => { if (!disposed) setError(message(error)); })
        .finally(() => { if (!disposed) setBusy(false); });
    };
    refresh();
    navigator.mediaDevices?.addEventListener('devicechange', refresh);
    return () => { disposed = true; navigator.mediaDevices?.removeEventListener('devicechange', refresh); };
  }, []);
  const run = async (action: () => Promise<MicrophoneList>) => {
    setBusy(true); setError(null);
    try { setList(await action()); } catch (error) { setError(message(error)); }
    finally { setBusy(false); }
  };
  const missing = list.selected_id && !list.devices.some(device => device.id === list.selected_id);
  return <>
    <Row title="Microphone device" detail="Used for your next recording. System default follows your operating system input. Stop recording before changing it." stacked>
      <select aria-label="Microphone device" value={list.selected_id ?? ''} disabled={disabled || busy}
        onChange={event => void run(() => selectMicrophone(event.target.value || null))}>
        <option value="">System default</option>
        {missing && <option value={list.selected_id!}>Selected microphone (unavailable)</option>}
        {list.devices.map(device => <option key={device.id} value={device.id}>{device.label}</option>)}
      </select>
      <button className="button ghost small" type="button" disabled={disabled || busy}
        onClick={() => void run(() => listMicrophones(!isTauri()))}>
        {isTauri() ? 'Refresh microphones' : 'Allow access and refresh'}
      </button>
    </Row>
    {typeof list.prefer_built_in === 'boolean' && (
      <Row title="Skip Bluetooth headset microphones"
        detail={`With System default selected, records from the Mac's own microphone while the system input is a Bluetooth headset. Opening a headset's microphone switches it to call mode, which sends about a second of silence before your words and lowers what you hear to call quality.${list.selected_id ? ' Applies only when System default is selected.' : ''}`}>
        <Toggle checked={list.prefer_built_in} label="Skip Bluetooth headset microphones" disabled={disabled || busy}
          onChange={enabled => void run(() => preferBuiltInMicrophone(enabled))} />
      </Row>
    )}
    {missing && <p className="notice alert" role="status">The selected microphone is unavailable. Reconnect it or choose another input before recording.</p>}
    {error && <p className="notice alert" role="alert">{error}</p>}
  </>;
}
