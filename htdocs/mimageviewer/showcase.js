(function () {
  function setText(root, selector, value) {
    var node = root.querySelector(selector);
    if (node) node.textContent = value || "";
  }

  function activate(showcase, button) {
    var stage = showcase.querySelector("[data-showcase-stage]");
    var link = showcase.querySelector("[data-showcase-link]");
    var main = showcase.querySelector("[data-showcase-main]");
    var alt = showcase.querySelector("[data-showcase-alt]");
    if (!stage || !link || !main || !alt || !button) return;

    var src = button.getAttribute("data-src");
    var altSrc = button.getAttribute("data-alt-src");
    var altText = button.getAttribute("data-alt") || button.textContent.trim();
    if (!src) return;

    main.src = src;
    main.alt = altText;
    link.href = button.getAttribute("data-full-src") || src;

    if (altSrc) {
      alt.src = altSrc;
      alt.alt = altText + "（切り替え後）";
      alt.hidden = false;
      stage.classList.add("is-animated");
    } else {
      alt.removeAttribute("src");
      alt.alt = "";
      alt.hidden = true;
      stage.classList.remove("is-animated");
    }

    setText(showcase, "[data-showcase-title]", button.getAttribute("data-title"));
    setText(showcase, "[data-showcase-desc]", button.getAttribute("data-desc"));

    showcase.querySelectorAll("[data-showcase-button]").forEach(function (item) {
      var selected = item === button;
      item.classList.toggle("is-active", selected);
      item.setAttribute("aria-selected", selected ? "true" : "false");
    });
  }

  function measureCaptionHeight(showcase) {
    var caption = showcase.querySelector(".showcase-caption");
    var title = showcase.querySelector("[data-showcase-title]");
    var desc = showcase.querySelector("[data-showcase-desc]");
    var buttons = Array.prototype.slice.call(showcase.querySelectorAll("[data-showcase-button]"));
    if (!caption || !title || !desc || !buttons.length) return;

    var currentTitle = title.textContent;
    var currentDesc = desc.textContent;
    var currentMinHeight = caption.style.minHeight;

    caption.style.minHeight = "";
    var maxHeight = 0;
    buttons.forEach(function (button) {
      title.textContent = button.getAttribute("data-title") || "";
      desc.textContent = button.getAttribute("data-desc") || "";
      maxHeight = Math.max(maxHeight, caption.getBoundingClientRect().height);
    });

    title.textContent = currentTitle;
    desc.textContent = currentDesc;
    caption.style.minHeight = maxHeight ? Math.ceil(maxHeight) + "px" : currentMinHeight;
  }

  function initShowcase(showcase) {
    var buttons = Array.prototype.slice.call(showcase.querySelectorAll("[data-showcase-button]"));
    if (!buttons.length) return;

    buttons.forEach(function (button) {
      button.addEventListener("mouseenter", function () { activate(showcase, button); });
      button.addEventListener("focus", function () { activate(showcase, button); });
      button.addEventListener("click", function () { activate(showcase, button); });
    });

    activate(showcase, showcase.querySelector("[data-showcase-button].is-active") || buttons[0]);
    measureCaptionHeight(showcase);
  }

  document.addEventListener("DOMContentLoaded", function () {
    document.querySelectorAll("[data-showcase]").forEach(initShowcase);
  });

  var resizePending = false;
  window.addEventListener("resize", function () {
    if (resizePending) return;
    resizePending = true;
    window.requestAnimationFrame(function () {
      document.querySelectorAll("[data-showcase]").forEach(measureCaptionHeight);
      resizePending = false;
    });
  });
})();
