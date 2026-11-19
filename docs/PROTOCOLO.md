# Telinhas — Protocolo de sinalização (rascunho)

> Deriva de DEC-07 a DEC-10, DEC-22 a DEC-26, DEC-32, DEC-35, DEC-42, DEC-46 a DEC-55. Nada aqui carrega vídeo: o servidor só repassa SDP/ICE e guarda o estado das salas.

## 1. Transporte

- WebSocket sobre TLS (`wss://`), via Cloudflare Tunnel (DEC-46).
- Mensagens em JSON, uma por frame de texto: `{ "v": 1, "type": "...", ... }`.
- `v` é a versão do protocolo. Servidor e cliente recusam versões que não conhecem, com `error` / `unsupported_version`.
- IP real do cliente: cabeçalho `CF-Connecting-IP`.
- Keepalive: ping/pong do WebSocket a cada 15 s; 45 s sem resposta contam como queda.

## 2. Identidade

- Cada participante tem um `peer_id` (UUID gerado pelo servidor na entrada) e um `session_token` (32 bytes aleatórios, base64url, gerado pelo **cliente** e enviado ao criar/entrar; DEC-42).
- O token só vale dentro da sala onde foi usado e deixa de valer quando a sala expira.
- Apelido: enviado pelo cliente. O servidor aplica o sufixo `(2)`, `(3)`… quando repetido (DEC-26) e devolve o apelido final.

## 3. Código de sala

- 8 símbolos de um alfabeto de 32 (Crockford base32 sem `I L O U`), RNG criptográfico (DEC-07).
- Exibido como `XXXX-XXXX`. O servidor aceita minúsculas, com ou sem hífen, e normaliza.
- Ao criar, o servidor sorteia até achar um código sem sala viva nem em espera (DEC-25).

## 4. Estados da sala

```
            create_room
  (nada) ───────────────► ATIVA ◄──────────┐
                            │              │ join / reconnect
                  último sai│              │ dentro dos 5 min
                            ▼              │
                         VAZIA ────────────┘
                            │ 5 min sem ninguém (DEC-23)
                            ▼
                         EXPIRADA  (código livre, DEC-25)
```

- **ATIVA:** há ao menos um participante conectado ou em reconexão.
- **VAZIA:** ninguém conectado. O código segue válido por 5 minutos. Quem tem `session_token` da sala reconecta como o mesmo participante, inclusive o administrador anterior (DEC-42).
- **EXPIRADA:** o estado é apagado.

Estado guardado por sala: código, lista de participantes (`peer_id`, apelido, `session_token`, ordem de entrada, conectado ou não), participantes "dormentes", administrador, flag `require_approval`, fila de pedidos pendentes e transmissões ativas.

**Dormentes (DEC-42 e DEC-55).** Quem é removido por estourar os 60 s não perde o token: fica guardado como dormente enquanto a sala existir. Se voltar com o mesmo token, retoma o `peer_id`, o apelido e a aprovação já concedida, sem novo pedido. Só retoma o papel de administrador se a sala estiver vazia no momento (se outro já assumiu, o papel não volta). Quem sai de propósito (`leave_room`) não vira dormente. Tudo isso some quando a sala expira.

**Sala vazia.** Quem entra numa sala sem participantes (e sem token conhecido) vira administrador, e não precisa de aprovação, pois não há quem aprove.

Participante que cai fica **reconectando** por 60 s; se não voltar, é removido (e o papel de administrador passa, se for o caso). A sala só vira VAZIA quando todos foram removidos.

## 5. Administrador (DEC-22)

- O criador começa como administrador.
- Se o administrador é **removido** (saiu de vez ou passou 60 s caído), o papel vai ao participante conectado mais antigo na sala. O servidor envia `admin_changed` a todos.
- Se o administrador original reconectar dentro dos 60 s, mantém o papel.
- Só o administrador envia `set_approval` e `approval_decision`.

## 6. Mensagens

### 6.1 Cliente → servidor

| `type` | Campos | Efeito |
|--------|--------|--------|
| `create_room` | `nickname`, `session_token`, `require_approval` (bool, padrão `false`) | Cria sala. Responde `room_joined`. |
| `join_room` | `code`, `nickname`, `session_token` | Entra (ou reconecta, se o token já existe na sala). |
| `set_nickname` | `nickname` | Troca de apelido (RF-15). |
| `set_approval` | `require_approval` | Só administrador. |
| `approval_decision` | `request_id`, `accept` (bool) | Só administrador. |
| `leave_room` | — | Saída definitiva. |
| `signal` | `to` (`peer_id`), `kind` (`offer`/`answer`/`ice`), `payload` | Repassado ao destino, sem interpretar. |
| `stream_state` | `stream_id`, `kind` (`screen`/`webcam`), `live` (bool), `meta` (resolução, fps, codec) | Anuncia/encerra transmissão (DEC-06, DEC-12). Repassado a todos. |
| `layer_request` | `to`, `stream_id`, `layer` (`original`/`economy`) | Espectador pede camada (DEC-04, DEC-05). Repassado ao transmissor. |
| `latency_report` | `samples` (`[{peer_id, rtt_ms}]`, até 64) | Repassado aos demais participantes, que montam o monitor de latência (DEC-19). |

### 6.2 Servidor → cliente

| `type` | Campos | Quando |
|--------|--------|--------|
| `room_joined` | `code`, `you` (`peer_id`, apelido final), `peers`, `admin`, `require_approval`, `streams`, `ice_servers` | Entrada ou reconexão concluída. `ice_servers` traz os relays (TURN) com credenciais temporárias, para quem não consegue conexão direta; vem vazio se o servidor não tem relay, e servidores antigos o omitem. |
| `approval_pending` | — | Para quem entra: aguardando o administrador. |
| `approval_request` | `request_id`, `nickname` | Para o administrador (RF-19). |
| `peer_joined` | `peer` (`peer_id`, `nickname`, `state`) | Alguém entrou. |
| `peer_left` | `peer_id`, `reason` (`left`/`timeout`) | Alguém saiu. Transmissões dele são encerradas antes, com `stream_state` `live: false`. |
| `peer_connection` | `peer_id`, `state` (`reconnecting`/`connected`) | Alimenta o estado "reconectando" (RF-21). |
| `peer_renamed` | `peer_id`, `nickname` | Troca de apelido. |
| `admin_changed` | `peer_id` | Troca de administrador. |
| `approval_changed` | `require_approval` | Opção alterada. |
| `signal` | `from`, `kind`, `payload` | Repasse. |
| `stream_state` | `from`, … | Repasse. |
| `layer_request` | `from`, … | Repasse. |
| `latency_report` | `from`, `samples` | Repasse a todos, menos ao remetente. |
| `room_over_capacity` | `count` | Informativo acima de 12; não bloqueia (DEC-24). |
| `error` | `code`, `message` | Ver §7. |

### 6.3 Fluxo de entrada

1. `join_room` com código válido e `require_approval = false` → `room_joined`; os outros recebem `peer_joined`.
2. Com `require_approval = true` → `approval_pending` ao candidato, `approval_request` ao administrador.
   - Aceito → `room_joined`. Recusado ou timeout de 60 s → `error` / `join_denied` e a conexão é encerrada (DEC-47).
   - Quem reconecta com `session_token` já conhecido (conectado, reconectando ou dormente) **não** passa de novo pela aprovação (DEC-42).
   - Se o administrador desliga a exigência (`set_approval` falso), todos os pedidos pendentes são aceitos na hora.
   - Se o administrador muda, os pedidos pendentes são reenviados ao novo (e também ao antigo, se ele reconectar).
   - Uma segunda conexão com o mesmo token substitui a primeira, que é encerrada.
3. Depois de `room_joined`, a negociação WebRTC vai por `signal`, e quem transmite envia a `offer` a cada participante (o espectador só responde com a `answer`), então não há colisão de ofertas. Um participante que entra depois recebe a oferta de quem já transmite. As descrições vão completas, com os candidatos ICE já coletados (sem trickle), então `ice` não é usado por enquanto.

## 7. Erros

| `code` | Significado |
|--------|-------------|
| `unsupported_version` | Versão de protocolo desconhecida. |
| `invalid_code` | Código malformado, inexistente ou expirado (mesma resposta nos três casos, para não vazar quais existem). |
| `rate_limited` | Limite de tentativas por IP (§8). Inclui `retry_after_s`. |
| `join_denied` | Recusado pelo administrador ou timeout. |
| `not_admin` | Ação restrita ao administrador. |
| `invalid_nickname` | Vazio ou além do limite (§9). |
| `bad_message` | JSON inválido ou campo faltando. |

## 8. Limite de tentativas por IP (DEC-48)

- Conta apenas `join_room` com código **inexistente, expirado ou malformado**. O bloqueio vale para todas as tentativas daquele IP, inclusive com o código certo.
- 10 falhas por minuto por IP; ao estourar, bloqueio de 5 minutos para `join_room` (resposta `rate_limited`).
- Cabeçalho `CF-Connecting-IP` só é confiável se a conexão vier do tunnel; o servidor ignora o cabeçalho em conexões diretas.
- Limites adicionais: criação de salas por IP (a definir após medir uso) e máximo de mensagens por segundo por conexão, com desconexão em caso de abuso.
- Com ≈ 2⁴⁰ códigos, 10 tentativas/min tornam inviável adivinhar um código de sala viva.

## 9. Limites

- Apelido: 1 a 24 caracteres, sem caracteres de controle, espaços nas pontas removidos.
- Mensagem: até 16 KiB (SDP cabe com folga).
- Participantes: sem teto no servidor; `room_over_capacity` a partir de 13 (DEC-24).

## 10. Implementação

Servidor em `signaling/` (DEC-54): `tokio` e `axum`, JSON com `serde`. Endpoints `GET /ws` (WebSocket) e `GET /healthz`. Porta por `TELINHAS_BIND` (padrão `127.0.0.1:8080`). `TELINHAS_TRUSTED_PROXIES` lista, separados por vírgula, os IPs de onde vem o `cloudflared` (padrão: só loopback); só conexões desses IPs podem informar o IP real pelo `CF-Connecting-IP`. Os testes de `signaling/tests/flow.rs` cobrem os fluxos deste documento, com prazos encurtados.

## 11. Questões ainda abertas

- Criação de salas por IP: limite numérico depende de uso real.
- Detalhe do `meta` de `stream_state` (campos exatos de resolução/fps/codec) depende da biblioteca WebRTC escolhida (DEC-38).
- Formato do `payload` de `signal` segue o que o `webrtc-rs` expõe.

## Relay (TURN)

Quem está atrás de um NAT simétrico ou de CGNAT não consegue conexão direta com os outros (o painel de diagnóstico mostra "NAT: SIMÉTRICO"). Para esses casos o servidor entrega, dentro de `room_joined`, os `ice_servers`: `urls`, `username` e `credential` do TURN da Cloudflare. O app só usa o relay quando o caminho direto falha. Não há dados de mídia no servidor de sinalização: o relay é da Cloudflare.

- O servidor liga o relay quando as variáveis `TELINHAS_TURN_KEY_ID` e `TELINHAS_TURN_TOKEN` existem no ambiente (a chave nunca entra no repositório nem é impressa). Sem elas, `ice_servers` vai vazio.
- As credenciais valem 24 horas e são renovadas a cada 4. Entram só os endereços UDP da porta 3478: a 443 por UDP não respondeu nos testes e travava a coleta de endereços.
- `TELINHAS_FORCE_RELAY=1` no app força passar só pelo relay, para testar.
- Teste real, que precisa da chave no ambiente: `cargo test -p telinhas-media --test relay -- --ignored --nocapture`.

