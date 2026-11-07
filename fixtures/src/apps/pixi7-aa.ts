// Fixture app: Pixi v7, antialias on, resolution uncapped, autoDensity on.
import { Application, Assets, Sprite } from "pixi.js";

const app = new Application<HTMLCanvasElement>({
  antialias: true,
  resolution: window.devicePixelRatio,
  autoDensity: true,
  backgroundColor: 0x101820,
  resizeTo: window,
});
document.body.appendChild(app.view);

Promise.all([
  Assets.load("assets/atlas-1024x1024.png"),
  Assets.load("assets/panel-1000x600.png"),
  Assets.load("assets/tile-512x512.jpg"),
]).then((textures) => {
  for (const texture of textures) app.stage.addChild(new Sprite(texture));
});
