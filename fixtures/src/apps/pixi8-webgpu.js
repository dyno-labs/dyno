// Fixture app: Pixi v8, antialias off, resolution capped at 2 through a
// binding, WebGPU preferred. Loaded from an external script file.
import { Application, Assets, Sprite } from 'pixi.js';
import { Howl } from 'howler';

const cappedResolution = Math.min(window.devicePixelRatio, 2);

async function boot() {
  const app = new Application();
  await app.init({
    antialias: false,
    resolution: cappedResolution,
    autoDensity: true,
    preference: 'webgpu',
    backgroundAlpha: 0,
    resizeTo: window,
  });
  document.body.appendChild(app.canvas);

  const symbols = await Assets.load('assets/symbols-1024x1024.ktx2');
  app.stage.addChild(new Sprite(symbols));

  const click = new Howl({ src: ['assets/click.mp3'] });
  click.play();
  fetch('//cdn.example.invalid/static/paytable.json').then((r) => r.json());
}

boot();
