import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'
import { Subscribe } from '@react-rxjs/core'
import App from './App'
import './index.css'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <Subscribe>
      <BrowserRouter>
        <App />
      </BrowserRouter>
    </Subscribe>
  </StrictMode>
)
