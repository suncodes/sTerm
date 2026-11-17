const paths = {
  terminal: '<path d="m5 6 5 6-5 6m8 0h6"/>',
  remote: '<rect x="3" y="3" width="14" height="11" rx="1.5"/><path d="M10 14v4m-4 0h7"/><rect x="15" y="10" width="6" height="11" rx="1.2"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v6m0-10v.5"/>',
  copy: '<rect x="8" y="8" width="12" height="13" rx="1.5"/><path d="M16 8V3H3v13h5"/>',
  refresh: '<path d="M20 10a8 8 0 1 0-.6 6M20 4v6h-6"/>',
  globe: '<circle cx="12" cy="12" r="9"/><ellipse cx="12" cy="12" rx="4" ry="9"/><path d="M3 12h18"/>',
  close: '<path d="m6 6 12 12M6 18 18 6"/>',
  check: '<path d="m5 12 4 4L19 6"/>',
};

export function remoteIcon(name: keyof typeof paths): string {
  return `<svg class="remote-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.65" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${paths[name]}</svg>`;
}
