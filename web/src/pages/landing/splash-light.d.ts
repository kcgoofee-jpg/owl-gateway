export type SplashLight = { draw: (seconds: number) => void; dispose: () => void };
export function createSplashLight(canvas: HTMLCanvasElement): Promise<SplashLight>;
export const posterUrl: string;
