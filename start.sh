#!/bin/bash
set -e
mkdir -p /app/data
# Seta a URL do banco para o volume persistente
export DATABASE_URL="sqlite:/app/data/monitor.db?mode=rwc"
# O scan NAO roda mais no boot: com auto_stop, cada primeiro visitante
# esperaria ~1 min. Popule manualmente quando precisar:
#   fly ssh console -C "monitor-diarios scan --since ... --until ..."
exec monitor-diarios serve --addr 0.0.0.0:8080
