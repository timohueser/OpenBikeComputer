/* Highlight only labelled fences; diagrams and unknown languages stay plain. */
(() => {
  hljs.registerAliases("asm", { languageName: "riscvasm" });
  document.querySelectorAll("pre.code > code").forEach((code) => {
    const language = code.className.match(/\blang-([\w-]+)\b/)?.[1];
    if (!language || language === "text" || !hljs.getLanguage(language)) return;
    code.classList.add(`language-${language}`);
    hljs.highlightElement(code);
  });
})();
