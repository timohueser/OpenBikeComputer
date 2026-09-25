/** @param {HTMLElement} header */
export function setupSiteNavigation(header) {
  const button = /** @type {HTMLButtonElement | null} */ (header.querySelector('button.site-menu-toggle'));
  const links = header.querySelector('.head-links');
  if (!button || !links) return;
  const mobile = matchMedia('(max-width: 760px)');
  function close() {
    header.classList.remove('site-menu-open');
    button.setAttribute('aria-expanded', 'false');
  }
  function toggle() {
    const open = header.classList.toggle('site-menu-open');
    button.setAttribute('aria-expanded', String(open));
  }
  /** @param {KeyboardEvent} event */
  function escape(event) {
    if (event.key === 'Escape' && header.classList.contains('site-menu-open')) {
      close();
      button.focus();
    }
  }
  /** @param {Event} event */
  function outside(event) {
    if (event.target instanceof Node && !header.contains(event.target)) close();
  }
  header.classList.add('site-menu-ready');
  button.addEventListener('click', toggle);
  links.addEventListener('click', close);
  mobile.addEventListener('change', close);
  document.addEventListener('keydown', escape);
  document.addEventListener('click', outside);
  document.addEventListener('focusin', outside);
  return () => {
    button.removeEventListener('click', toggle);
    links.removeEventListener('click', close);
    mobile.removeEventListener('change', close);
    document.removeEventListener('keydown', escape);
    document.removeEventListener('click', outside);
    document.removeEventListener('focusin', outside);
  };
}
