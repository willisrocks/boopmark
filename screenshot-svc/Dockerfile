FROM mcr.microsoft.com/playwright:v1.50.0-noble

WORKDIR /app
COPY package.json .
RUN npm install --omit=dev
RUN npx playwright install chromium

COPY index.js .

EXPOSE 3001
CMD ["node", "index.js"]
