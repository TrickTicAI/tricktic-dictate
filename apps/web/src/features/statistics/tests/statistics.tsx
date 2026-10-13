import '@fontsource/inter/400.css';
import '@fontsource/inter/600.css';
import { createRoot } from 'react-dom/client';
import { Statistics } from '../Statistics';
import { TopBar } from '../../shell/TopBar';
import { applyTheme } from '../../shell/theme';
import { themeById } from '../../../themes';
import { recordings, saved } from './fixtures';
import '../../../styles.css';
const params = new URLSearchParams(location.search);
applyTheme(themeById(params.get('theme') ?? 'paper'));
if(params.has('rtl')) document.documentElement.dir='rtl';
createRoot(document.getElementById('root')!).render(<div className="shell" style={{gridTemplateRows:'minmax(0,1fr)'}}><div className="shell-body">
<TopBar tab="statistics" onTab={() => {}} onSettings={() => {}} settingsOpen={false} status={{tone:'online',title:'Ready',detail:'Ready',action:'Open models'}} onStatusAction={() => {}} onEngineSettings={() => {}} />
<Statistics recordings={params.has('empty') ? [] : recordings} saved={saved} loading={false} onOpenSession={id => {document.body.dataset.opened = id;}}
  onOpenSettings={section => {document.body.dataset.settings = section;}} onOpenHistory={filter => {document.body.dataset.history = filter;}} />
</div></div>);
