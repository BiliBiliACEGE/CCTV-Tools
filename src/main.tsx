import ReactDOM from 'react-dom/client';

import App from './App';
import { installShellHardening } from './lib/harden';
import './styles.css';

// 打包产物里关掉右键菜单与开发者工具快捷键（开发模式自动跳过）
installShellHardening();

ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(<App />);
