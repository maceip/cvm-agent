(() => {
  "use strict";

  const $$ = (selector, root = document) => Array.from(root.querySelectorAll(selector));

  const initTabs = () => {
    $$("[data-tab]").forEach((tab) => {
      tab.tabIndex = tab.getAttribute("aria-selected") === "true" ? 0 : -1;

      const activate = () => {
        const owner = tab.closest('[is-~="tabs"]') || document;
        const target = tab.dataset.tab;

        $$("[data-tab]", owner).forEach((item) => {
          const selected = item === tab;
          item.setAttribute("aria-selected", String(selected));
          item.tabIndex = selected ? 0 : -1;
        });

        $$("[data-panel]", owner).forEach((panel) => {
          const active = panel.dataset.panel === target;
          panel.hidden = !active;
          panel.classList.toggle("is-active", active);
        });
      };

      tab.addEventListener("click", activate);
      tab.addEventListener("keydown", (event) => {
        const owner = tab.closest('[is-~="tabs"]') || document;
        const tabs = $$("[data-tab]", owner);
        const index = tabs.indexOf(tab);
        const keyMap = {
          ArrowLeft: index - 1,
          ArrowRight: index + 1,
          Home: 0,
          End: tabs.length - 1
        };

        if (!(event.key in keyMap)) return;

        event.preventDefault();
        const nextIndex = (keyMap[event.key] + tabs.length) % tabs.length;
        tabs[nextIndex].focus();
        tabs[nextIndex].click();
      });
    });
  };

  const initTilt = () => {
    $$("[data-tilt]").forEach((card) => {
      const max = Number(card.dataset.tiltMax || 10);

      card.addEventListener("pointermove", (event) => {
        const rect = card.getBoundingClientRect();
        const x = (event.clientX - rect.left) / rect.width;
        const y = (event.clientY - rect.top) / rect.height;
        const rotateY = (x - 0.5) * max * 2;
        const rotateX = (0.5 - y) * max * 2;

        card.style.setProperty("--tilt-x", `${rotateX.toFixed(2)}deg`);
        card.style.setProperty("--tilt-y", `${rotateY.toFixed(2)}deg`);
        card.style.setProperty("--tilt-glare-x", `${(x * 100).toFixed(2)}%`);
        card.style.setProperty("--tilt-glare-y", `${(y * 100).toFixed(2)}%`);
      });

      card.addEventListener("pointerleave", () => {
        card.style.setProperty("--tilt-x", "0deg");
        card.style.setProperty("--tilt-y", "0deg");
        card.style.setProperty("--tilt-glare-x", "50%");
        card.style.setProperty("--tilt-glare-y", "50%");
      });
    });
  };

  const updateDots = (carousel, index) => {
    $$("[is-~='carousel-dot']", carousel).forEach((dot, dotIndex) => {
      dot.classList.toggle("active", dotIndex === index);
      dot.setAttribute("aria-selected", String(dotIndex === index));
    });
  };

  const initCarousel = () => {
    $$("[data-carousel]").forEach((carousel) => {
      const track = carousel.querySelector("[data-carousel-track]");
      const slides = $$("[is-~='carousel-slide']", carousel);
      const dots = carousel.querySelector("[data-carousel-dots]");
      const prev = carousel.querySelector("[data-carousel-prev]");
      const next = carousel.querySelector("[data-carousel-next]");
      let index = 0;

      if (!track || slides.length === 0 || !dots) return;

      slides.forEach((_, slideIndex) => {
        const item = document.createElement("li");
        const button = document.createElement("button");
        button.type = "button";
        button.setAttribute("is-", "carousel-dot");
        button.setAttribute("aria-label", `Go to slide ${slideIndex + 1}`);
        button.addEventListener("click", () => {
          index = slideIndex;
          slides[index].scrollIntoView({ behavior: "smooth", block: "nearest", inline: "start" });
          updateDots(carousel, index);
        });
        item.appendChild(button);
        dots.appendChild(item);
      });

      const go = (direction) => {
        index = (index + direction + slides.length) % slides.length;
        slides[index].scrollIntoView({ behavior: "smooth", block: "nearest", inline: "start" });
        updateDots(carousel, index);
      };

      prev?.addEventListener("click", () => go(-1));
      next?.addEventListener("click", () => go(1));

      track.addEventListener("scroll", () => {
        const trackLeft = track.getBoundingClientRect().left;
        let closest = 0;
        let closestDistance = Number.POSITIVE_INFINITY;

        slides.forEach((slide, slideIndex) => {
          const distance = Math.abs(slide.getBoundingClientRect().left - trackLeft);
          if (distance < closestDistance) {
            closest = slideIndex;
            closestDistance = distance;
          }
        });

        index = closest;
        updateDots(carousel, index);
      }, { passive: true });

      updateDots(carousel, index);
    });
  };

  document.addEventListener("DOMContentLoaded", () => {
    initTabs();
    initTilt();
    initCarousel();
  });
})();
