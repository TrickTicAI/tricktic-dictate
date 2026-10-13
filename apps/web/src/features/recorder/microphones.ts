import { invoke, isTauri } from '../desktop-session/runtime/native';

/** `prefer_built_in` is absent or null where the platform offers no such choice. */
export type MicrophoneList = { devices: { id: string; label: string }[]; selected_id: string | null; prefer_built_in?: boolean | null };
const preferenceKey = 'writer.microphone.device';
export function browserMicrophone(): string | null { return localStorage.getItem(preferenceKey); }
export function microphoneConstraints(): MediaStreamConstraints {
  const id = browserMicrophone();
  return { audio: id ? { deviceId: { exact: id } } : true };
}
export async function listMicrophones(requestLabels = false): Promise<MicrophoneList> {
  if (isTauri()) return invoke('list_microphones');
  if (!navigator.mediaDevices?.enumerateDevices) throw new Error('Microphone selection requires localhost or HTTPS and a supported browser.');
  if (requestLabels) {
    // Permission is requested only by an explicit user click; listing settings
    // never opens a microphone. Release this temporary stream immediately.
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    stream.getTracks().forEach(track => track.stop());
  }
  const inputs = (await navigator.mediaDevices.enumerateDevices()).filter(device => device.kind === 'audioinput' && device.deviceId && device.deviceId !== 'default');
  return { devices: inputs.map((device, index) => ({ id: device.deviceId, label: device.label || `Microphone ${index + 1}` })), selected_id: browserMicrophone() };
}
export async function selectMicrophone(deviceId: string | null): Promise<MicrophoneList> {
  if (isTauri()) return invoke('select_microphone', { deviceId });
  const devices = await listMicrophones();
  if (deviceId && !devices.devices.some(device => device.id === deviceId)) throw new Error('That microphone is no longer available. Refresh the device list.');
  if (deviceId) localStorage.setItem(preferenceKey, deviceId);
  else localStorage.removeItem(preferenceKey);
  return { ...devices, selected_id: deviceId };
}
/** macOS: with System default selected, record from the built-in microphone while the system input is a Bluetooth headset. */
export function preferBuiltInMicrophone(enabled: boolean): Promise<MicrophoneList> {
  return invoke('microphone_prefer_built_in', { enabled });
}
