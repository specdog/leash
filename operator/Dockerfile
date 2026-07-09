FROM node:22-alpine

WORKDIR /app
COPY server.js /app/server.js
COPY fleet.example.json /app/fleet.example.json
COPY public /app/public

ENV PORT=8787
EXPOSE 8787

CMD ["node", "/app/server.js"]
