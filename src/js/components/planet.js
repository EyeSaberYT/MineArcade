// Planets for the Play page's orbit: each instance is drawn as a small world.
//
// A world is an SVG: a surface for its kind (gas bands, ice, lava, oceans, crystal)
// clipped to a disc, then one shared light pass on top (night side, coloured
// atmosphere at the rim, a soft highlight). Gas giants wear a ring whose back half
// is drawn behind the disc and front half over it. Surfaces come from SVG noise
// seeded per instance, so two worlds of the same kind still look different.
import { prefs } from '../store.js';
import { raw } from '../ui.js';

export const PLANET_KINDS = [
  { id: 'gas', label: 'Gas giant', atmo: '#b9a8ff', glow: 'rgba(170, 150, 255, 0.55)', ring: true },
  { id: 'ice', label: 'Ice world', atmo: '#a8e4ff', glow: 'rgba(140, 210, 255, 0.5)' },
  { id: 'lava', label: 'Lava world', atmo: '#ff9a5a', glow: 'rgba(255, 140, 70, 0.5)' },
  { id: 'ocean', label: 'Ocean world', atmo: '#7fc8ff', glow: 'rgba(120, 200, 255, 0.45)' },
  { id: 'crystal', label: 'Crystal world', atmo: '#ff9ad0', glow: 'rgba(255, 120, 190, 0.5)' },
];

const kindById = (id) => PLANET_KINDS.find((k) => k.id === id) ?? PLANET_KINDS[0];

/** A stable small number from an instance id. */
function hash(text) {
  let h = 2166136261;
  for (const char of text) h = Math.imul(h ^ char.charCodeAt(0), 16777619);
  return h >>> 0;
}

/** The world an instance is drawn as: picked by the player, else the Orbit Client gas giant, else from its id. */
export function planetOf(instance) {
  const seed = (hash(instance.id) % 97) + 1;
  const picked = prefs.get(`planet.${instance.id}`, null);
  if (picked) return { kind: kindById(picked), seed };
  if (instance.tags?.includes('orbit-client')) return { kind: kindById('gas'), seed: 7 };
  // The loader suggests a world: vanilla is Earth-like, Forge packs are industrial lava,
  // Quilt is crystal. Fabric instances get any of them but the gas giant, which marks
  // Orbit Client unless picked.
  const byLoader = { vanilla: 'ocean', forge: 'lava', neoforge: 'lava', quilt: 'crystal' }[instance.loader];
  if (byLoader) return { kind: kindById(byLoader), seed };
  const others = ['ice', 'crystal', 'lava', 'ocean'];
  return { kind: kindById(others[hash(instance.id) % others.length]), seed };
}

export function setPlanetKind(instanceId, kindId) {
  prefs.set(`planet.${instanceId}`, kindId);
}

let uid = 0;

function surface(kind, id, seed) {
  switch (kind) {
    case 'gas':
      return {
        defs: `<linearGradient id="${id}b" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0" stop-color="#e9e2ff"/><stop offset=".12" stop-color="#b6a2ff"/><stop offset=".2" stop-color="#7d62e8"/>
            <stop offset=".3" stop-color="#d9ccff"/><stop offset=".38" stop-color="#9a82ff"/><stop offset=".47" stop-color="#5b43c4"/>
            <stop offset=".55" stop-color="#c3b2ff"/><stop offset=".63" stop-color="#8f6cf0"/><stop offset=".72" stop-color="#efe9ff"/>
            <stop offset=".8" stop-color="#7a5ad8"/><stop offset=".9" stop-color="#b9a5ff"/><stop offset="1" stop-color="#4a339e"/></linearGradient>
          <filter id="${id}w" x="-10%" y="-10%" width="120%" height="120%"><feTurbulence type="fractalNoise" baseFrequency=".012 .09" numOctaves="3" seed="${seed}"/>
            <feDisplacementMap in="SourceGraphic" scale="9" xChannelSelector="R" yChannelSelector="G"/></filter>`,
        body: `<g filter="url(#${id}w)"><rect x="-6" y="-6" width="112" height="112" fill="url(#${id}b)"/>
            <ellipse cx="64" cy="61" rx="11" ry="5.5" fill="#f2c4ff" opacity=".85"/><ellipse cx="64" cy="61" rx="6" ry="2.6" fill="#a35bd8"/></g>`,
      };
    case 'ice':
      return {
        defs: `<radialGradient id="${id}b" cx=".4" cy=".35" r=".8"><stop offset="0" stop-color="#f4fbff"/><stop offset=".6" stop-color="#9fd6f5"/><stop offset="1" stop-color="#3f86c4"/></radialGradient>
          <filter id="${id}c"><feTurbulence type="turbulence" baseFrequency=".09" numOctaves="2" seed="${seed}"/>
            <feColorMatrix values="0 0 0 0 .25  0 0 0 0 .5  0 0 0 0 .75  0 0 0 -9 2.2"/></filter>`,
        body: `<rect width="100" height="100" fill="url(#${id}b)"/><rect width="100" height="100" filter="url(#${id}c)" opacity=".7"/>
          <ellipse cx="50" cy="4" rx="40" ry="14" fill="#fff" opacity=".9"/>`,
      };
    case 'lava':
      return {
        defs: `<radialGradient id="${id}b" cx=".4" cy=".35" r=".8"><stop offset="0" stop-color="#6b3a2a"/><stop offset="1" stop-color="#231210"/></radialGradient>
          <filter id="${id}c"><feTurbulence type="turbulence" baseFrequency=".05" numOctaves="3" seed="${seed}"/>
            <feColorMatrix values="0 0 0 0 1  0 0 0 0 .55  0 0 0 0 .15  0 0 0 -14 3.1"/><feGaussianBlur stdDeviation=".5"/></filter>`,
        body: `<rect width="100" height="100" fill="url(#${id}b)"/><rect width="100" height="100" filter="url(#${id}c)"/>
          <g fill="#120807" opacity=".55"><circle cx="30" cy="62" r="7"/><circle cx="68" cy="30" r="5"/><circle cx="72" cy="70" r="4"/></g>`,
      };
    case 'ocean':
      return {
        defs: `<radialGradient id="${id}b" cx=".4" cy=".35" r=".8"><stop offset="0" stop-color="#5fc0ff"/><stop offset="1" stop-color="#0b3f7a"/></radialGradient>
          <filter id="${id}l"><feTurbulence type="fractalNoise" baseFrequency=".035" numOctaves="4" seed="${seed}"/>
            <feColorMatrix values="0 0 0 0 .3  0 0 0 0 .66  0 0 0 0 .34  0 0 0 -12 6.3"/></filter>
          <filter id="${id}k"><feTurbulence type="fractalNoise" baseFrequency=".05 .12" numOctaves="3" seed="${seed + 20}"/>
            <feColorMatrix values="0 0 0 0 1  0 0 0 0 1  0 0 0 0 1  0 0 0 -9 4.9"/></filter>`,
        body: `<rect width="100" height="100" fill="url(#${id}b)"/><rect width="100" height="100" filter="url(#${id}l)"/>
          <rect width="100" height="100" filter="url(#${id}k)" opacity=".85"/>`,
      };
    default:
      return {
        defs: `<linearGradient id="${id}b" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#ffd6ec"/><stop offset=".5" stop-color="#e2569a"/><stop offset="1" stop-color="#5a1a4a"/></linearGradient>`,
        body: `<rect width="100" height="100" fill="url(#${id}b)"/>
          <g opacity=".55" stroke="#fff" stroke-width=".6" fill="none"><path d="M0 30 L40 10 L70 40 L100 20 M10 100 L35 55 L70 40 L90 100 M35 55 L0 60 M70 40 L100 65"/></g>
          <g fill="#fff"><path d="M40 10 L70 40 L35 55Z" opacity=".22"/><path d="M70 40 L100 65 L90 100Z" opacity=".12"/><path d="M0 60 L35 55 L10 100Z" opacity=".1"/></g>`,
      };
  }
}

function ring(id, half) {
  const clip = half === 'back' ? '<rect x="-60" y="-40" width="220" height="90"/>' : '<rect x="-60" y="50" width="220" height="90"/>';
  return `<svg class="planet-layer" viewBox="0 0 100 100" aria-hidden="true"><defs><clipPath id="${id}${half}">${clip}</clipPath>
    <linearGradient id="${id}r${half}" x1="0" x2="1"><stop offset="0" stop-color="#ffe6c4" stop-opacity=".15"/><stop offset=".5" stop-color="#fff3df" stop-opacity=".95"/><stop offset="1" stop-color="#ffd7a8" stop-opacity=".25"/></linearGradient></defs>
    <g clip-path="url(#${id}${half})" transform="rotate(-16 50 50)"><ellipse cx="50" cy="50" rx="86" ry="20" fill="none" stroke="url(#${id}r${half})" stroke-width="7"/>
    <ellipse cx="50" cy="50" rx="74" ry="16" fill="none" stroke="#e9d3ff" stroke-opacity=".35" stroke-width="2.5"/></g></svg>`;
}

/** The planet for an instance, as markup to put in a sized box (it fills the box). */
export function planet(instance) {
  const { kind, seed } = planetOf(instance);
  const id = `pl${uid++}`;
  const { defs, body } = surface(kind.id, id, seed);
  return raw(`<span class="planet-halo" style="--glow:${kind.glow}"></span>${kind.ring ? ring(id, 'back') : ''}
    <svg class="planet-layer" viewBox="0 0 100 100" aria-hidden="true"><defs>
      <clipPath id="${id}clip"><circle cx="50" cy="50" r="50"/></clipPath>
      <radialGradient id="${id}sh" cx=".3" cy=".28" r=".95"><stop offset=".35" stop-color="#000" stop-opacity="0"/><stop offset=".72" stop-color="#05040f" stop-opacity=".55"/><stop offset="1" stop-color="#05040f" stop-opacity=".95"/></radialGradient>
      <radialGradient id="${id}rim" cx=".5" cy=".5" r=".5"><stop offset=".86" stop-color="${kind.atmo}" stop-opacity="0"/><stop offset="1" stop-color="${kind.atmo}" stop-opacity=".75"/></radialGradient>
      <radialGradient id="${id}sp" cx=".32" cy=".26" r=".22"><stop offset="0" stop-color="#fff" stop-opacity=".55"/><stop offset="1" stop-color="#fff" stop-opacity="0"/></radialGradient>
      ${defs}</defs>
      <g clip-path="url(#${id}clip)">${body}
        <circle cx="50" cy="50" r="50" fill="url(#${id}rim)"/><circle cx="50" cy="50" r="50" fill="url(#${id}sh)"/><circle cx="50" cy="50" r="50" fill="url(#${id}sp)"/></g>
    </svg>${kind.ring ? ring(id, 'front') : ''}`);
}
