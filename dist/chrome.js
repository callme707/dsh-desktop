const invoke = window.__TAURI_INTERNALS__?.invoke;

const statusLabel = document.querySelector('#kernel-status-label');
const updateButton = document.querySelector('#check-update');
const maximizeButton = document.querySelector('#maximize');
const maximizeIcon = document.querySelector('#maximize-icon');

const kernelStates = {
  starting: { label: '正在启动', updateEnabled: false },
  ready: { label: '内核已就绪', updateEnabled: true },
  checking: { label: '正在检查更新', updateEnabled: false },
  available: { label: '发现新版本', updateEnabled: false },
  updating: { label: '正在更新内核', updateEnabled: false },
};

window.setKernelState = (state) => {
  const next = kernelStates[state] ?? kernelStates.starting;
  document.documentElement.dataset.kernelState = state in kernelStates ? state : 'starting';
  statusLabel.textContent = next.label;
  updateButton.disabled = !next.updateEnabled;
};

window.setMaximized = (maximized) => {
  const isMaximized = Boolean(maximized);
  maximizeButton.ariaLabel = isMaximized ? '还原' : '最大化';
  maximizeButton.title = isMaximized ? '还原' : '最大化';
  maximizeIcon.src = isMaximized ? './assets/icons/copy.svg' : './assets/icons/square.svg';
};

window.setWindowActive = (active) => {
  document.documentElement.toggleAttribute('data-window-inactive', !active);
};

async function windowAction(action) {
  if (!invoke) return;
  try {
    const maximized = await invoke('chrome_window_action', { action });
    if (action === 'toggle-maximize') window.setMaximized(maximized);
  } catch (error) {
    console.error(`窗口操作失败：${action}`, error);
  }
}

for (const dragZone of document.querySelectorAll('[data-drag-zone]')) {
  dragZone.addEventListener('mousedown', (event) => {
    if (event.button !== 0 || event.target.closest('button')) return;
    windowAction(event.detail === 2 ? 'toggle-maximize' : 'start-dragging');
  });
}

document.querySelector('#minimize').addEventListener('click', () => windowAction('minimize'));
maximizeButton.addEventListener('click', () => windowAction('toggle-maximize'));
document.querySelector('#close').addEventListener('click', () => windowAction('close'));

updateButton.addEventListener('click', async () => {
  if (!invoke || updateButton.disabled) return;
  try {
    await invoke('check_dsh_update');
  } catch (error) {
    console.error('检查 dsh 更新失败', error);
    window.setKernelState('ready');
  }
});

window.setKernelState('starting');

if (invoke) {
  invoke('chrome_snapshot')
    .then(([state, maximized]) => {
      window.setKernelState(state);
      window.setMaximized(maximized);
    })
    .catch((error) => console.error('读取窗口状态失败', error));
}
