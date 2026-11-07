package example.app;
import org.junit.Test;
import static org.junit.Assert.assertTrue;
public class Behavior {
    @Test public void runtimeAndApi() throws Exception { assertTrue(Answer.correct()); }
}
