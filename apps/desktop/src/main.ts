import { mount } from 'svelte'
// xterm's own stylesheet hides its input textarea and lays out the terminal.
import '@xterm/xterm/css/xterm.css'
import './app.css'
import App from './App.svelte'

const app = mount(App, {
  target: document.getElementById('app')!,
})

export default app
