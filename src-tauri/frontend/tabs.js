// Sidebar navigation between the sections.
//
// Selection is a class rather than an inline display, so the stylesheet owns
// what a shown tab looks like and can animate the change.

const sections = document.querySelectorAll('.section');
const tabContents = document.querySelectorAll('.tab-content');

/// Shows one tab and marks its sidebar entry, by the tab's element id.
export function selectTab(tabName) {
  sections.forEach(section => {
    if (section.dataset.tab === tabName) {
      section.setAttribute('aria-current', 'page');
    } else {
      section.removeAttribute('aria-current');
    }
  });
  tabContents.forEach(content => {
    content.classList.toggle('active', content.id === tabName);
  });
}

sections.forEach(section => {
  section.addEventListener('click', () => selectTab(section.dataset.tab));
});
