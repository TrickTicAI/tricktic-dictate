import { createRoot } from 'react-dom/client';
import { TextModels } from '../TextModels';
import '../modes.css';

declare global {
  interface Window {
    navigated: string[];
  }
}
window.navigated = [];

// What the settings modal owns: where its links go, API keys included.

createRoot(document.getElementById('root')!).render(
  <TextModels
    onOpenSection={(section) => window.navigated.push(`section:${section}`)}
    onOpenKeys={(provider) => window.navigated.push(`keys:${provider}`)}
  />,
);
