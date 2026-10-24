const task = await taskSpace("hb-github-20261024");
const page = task.page("p1");
await page.goto("https://github.com/");
await page.waitForTimeout(2500);
const info = await page.evaluate(`(() => {
  const user = document.querySelector('meta[name="user-login"]')?.content ?? null;
  const links = [...document.querySelectorAll('a')].map((a) => a.getAttribute('href')).filter(Boolean);
  const signIn = links.some((href) => href.includes('/login'));
  const repos = links.filter((href) => /^\\/[^/]+\\/[^/]+$/.test(href)).slice(0, 15);
  return { url: location.href, user, signIn, repos };
})()`);
console.log(JSON.stringify(info, null, 2));
await task.finish({ keep: [] });
