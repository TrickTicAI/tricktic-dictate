import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import '@fontsource/inter/400.css';
import '@fontsource/inter/500.css';
import '@fontsource/inter/600.css';
// Inter does not cover Hebrew adequately for body text. Assistant carries both
// scripts and is bundled rather than fetched, because the desktop app runs
// against a private origin and has to work offline.
import '@fontsource/assistant/400.css';
import '@fontsource/assistant/600.css';
import './styles.css';
import { App } from './App';
import { AuthGate } from './features/auth/AuthGate';
import { applyTheme, storedThemeId } from './features/shell/theme';
import { themeById } from './themes';

// Applied before the first paint so the window never flashes the wrong palette.
applyTheme(themeById(storedThemeId()));

createRoot(document.getElementById('root')!).render(<StrictMode><AuthGate><App /></AuthGate></StrictMode>);
