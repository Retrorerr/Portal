import java.awt.*;
import javax.swing.*;

public class JavaProbe {
    private static int frames;
    private static long start;
    private static final boolean explicitSync = "1".equals(System.getenv("PORTAL_PROBE_SYNC"));

    public static void main(String[] args) throws Exception {
        int milliseconds = (int)(Double.parseDouble(args[0]) * 1000);
        SwingUtilities.invokeAndWait(() -> {
            JFrame window = new JFrame("Portal compatibility: Java Swing");
            window.setDefaultCloseOperation(JFrame.DISPOSE_ON_CLOSE);
            JPanel canvas = new JPanel() {
                protected void paintComponent(Graphics graphics) {
                    super.paintComponent(graphics);
                    if (start == 0) start = System.nanoTime();
                    frames++;
                    graphics.setColor(new Color(0xe8794a));
                    graphics.fillRect((frames * 6) % 600, 80, 100, 100);
                }
            };
            canvas.setPreferredSize(new Dimension(720, 360));
            window.add(new JTextField("Native Java Swing / Unicode: 日本語 café"), BorderLayout.NORTH);
            window.add(canvas, BorderLayout.CENTER);
            window.pack();
            window.setVisible(true);
            Timer animation = new Timer(16, event -> {
                canvas.repaint();
                if (explicitSync) SwingUtilities.invokeLater(() -> Toolkit.getDefaultToolkit().sync());
            });
            animation.start();
            Timer finish = new Timer(milliseconds, event -> {
                animation.stop();
                double scale = window.getGraphicsConfiguration().getDefaultTransform().getScaleX();
                System.out.printf(java.util.Locale.ROOT,
                    "{\"toolkit\":\"Swing\",\"backend\":\"%s\",\"version\":\"%s\",\"frames\":%d,\"scale\":%.3f,\"explicit_sync\":%s,\"flow_start_ns\":%d,\"flow_end_ns\":%d}%n",
                    Toolkit.getDefaultToolkit().getClass().getName(), System.getProperty("java.version"),
                    frames, scale, explicitSync, start, System.nanoTime());
                window.dispose();
                System.exit(0);
            });
            finish.setRepeats(false);
            finish.start();
        });
    }
}
