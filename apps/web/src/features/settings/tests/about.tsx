import { createRoot } from 'react-dom/client';
import { About } from '../About';
import '../../../styles.css';
import { applyTheme } from '../../shell/theme';
import { themeById } from '../../../themes';

// `?browser` renders the self-hosted client; otherwise the desktop window, whose
// shell the spec fakes. `?theme=` picks the app theme.
const params = new URLSearchParams(location.search);
applyTheme(themeById(params.get('theme')));
createRoot(document.getElementById('root')!).render(
  <div style={{ maxWidth: 760, padding: 24 }}><About integrated={!params.has('browser')} version="0.22.0" /></div>,
);
