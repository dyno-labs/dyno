// Fixture app: Pixi v7, options object in a binding, no antialias key (the
// default is off), resolution not capped, fixed 720 x 1280 stage.
import { Application, Assets, Sprite } from "pixi.js";

const options = {
  resolution: window.devicePixelRatio || 1,
  backgroundAlpha: 0,
  width: 720,
  height: 1280,
};
const app = new Application<HTMLCanvasElement>(options);
document.body.appendChild(app.view);

Assets.load("assets/bg-720x1280.webp").then((texture) => app.stage.addChild(new Sprite(texture)));
